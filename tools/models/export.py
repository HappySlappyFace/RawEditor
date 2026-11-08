#!/usr/bin/env python3
"""Convert the AI models RawEditor downloads into ONNX.

A one-off developer step: the app never runs this. Its output is uploaded to
the repo's `models-v1` GitHub release and listed (with SHA-256s) in
`src/ml/models.rs`. See README.md next to this file for the setup.

    python export.py --sam2-repo ../sam2 --sam2-ckpt sam2.1_hiera_tiny.pt \
                     --pmrid-repo ../PMRID --out dist/

Each exported graph is run through ONNX Runtime and compared with PyTorch on
random inputs; the script fails if they disagree.

Licences: SAM 2.1 (code and checkpoints) and PMRID are Apache-2.0. The ONNX
files are modified forms of those works (re-serialised graphs, the SAM
decoder wrapper below); NOTICE in the release says so.
"""
import argparse
import hashlib
import json
import sys
from pathlib import Path

import numpy as np
import onnxruntime as ort
import torch
import torch.nn as nn

OPSET = 17


def check(name, session, feeds, expected, atol):
    outs = session.run(None, {k: v.numpy() for k, v in feeds.items()})
    for i, (got, want) in enumerate(zip(outs, expected)):
        want = want.detach().numpy()
        err = float(np.max(np.abs(got - want)))
        print(f"  {name} output {i}: shape {got.shape}, max |onnx - torch| = {err:.2e}")
        if err > atol:
            sys.exit(f"{name}: ONNX output {i} differs from PyTorch by {err} (> {atol})")


# --------------------------------------------------------------------------
# SAM 2.1
# --------------------------------------------------------------------------

class SamEncoder(nn.Module):
    """1024² normalised RGB -> (image_embed, high_res_0, high_res_1).

    Mirrors `SAM2ImagePredictor.set_image` after its transforms: the caller
    resizes to 1024² (squashing, not padding — SAM 2's own transform) and
    normalises with ImageNet mean/std.
    """

    def __init__(self, model):
        super().__init__()
        self.model = model
        self.sizes = [(256, 256), (128, 128), (64, 64)]

    def forward(self, image):
        backbone_out = self.model.forward_image(image)
        _, feats, _, _ = self.model._prepare_backbone_features(backbone_out)
        if self.model.directly_add_no_mem_embed:
            feats[-1] = feats[-1] + self.model.no_mem_embed
        out = [
            f.permute(1, 2, 0).reshape(1, -1, h, w)
            for f, (h, w) in zip(feats, self.sizes)
        ]
        return out[2], out[0], out[1]


class SamDecoder(nn.Module):
    """Prompt encoder + mask decoder, returning ALL four mask tokens.

    Inputs (points already in the 1024² frame; labels 1 = add, 0 = subtract,
    2/3 = box corners). A padding point (label -1) is appended exactly as
    `PromptEncoder._embed_points(pad=True)` does, since boxes are passed as
    points. `has_mask_input` selects between the previous low-res logits and
    the learned no-mask embedding, so one graph serves the first click and
    every refinement. The app chooses among the four masks (SAM's multimask
    rule) itself.
    """

    def __init__(self, model):
        super().__init__()
        self.model = model
        self.pe = model.sam_prompt_encoder
        self.dec = model.sam_mask_decoder

    def forward(self, image_embed, high_res_0, high_res_1, point_coords, point_labels,
                mask_input, has_mask_input):
        pe = self.pe
        pad_pt = torch.zeros((1, 1, 2), dtype=point_coords.dtype)
        pad_lb = -torch.ones((1, 1), dtype=point_labels.dtype)
        coords = torch.cat([point_coords, pad_pt], dim=1) + 0.5
        labels = torch.cat([point_labels, pad_lb], dim=1)
        emb = pe.pe_layer.forward_with_coords(coords, pe.input_image_size)
        lab = labels.unsqueeze(-1)
        emb = torch.where(lab == -1, torch.zeros_like(emb) + pe.not_a_point_embed.weight, emb)
        for i in range(4):
            emb = torch.where(lab == i, emb + pe.point_embeddings[i].weight, emb)

        h = has_mask_input.reshape(1, 1, 1, 1)
        dense = h * pe.mask_downscaling(mask_input) + (1 - h) * pe.no_mask_embed.weight.reshape(1, -1, 1, 1)

        masks, iou, _, _ = self.dec.predict_masks(
            image_embeddings=image_embed,
            image_pe=pe.get_dense_pe(),
            sparse_prompt_embeddings=emb,
            dense_prompt_embeddings=dense,
            repeat_image=False,
            high_res_features=[high_res_0, high_res_1],
        )
        return torch.clamp(masks, -32.0, 32.0), iou


def export_sam(repo: Path, ckpt: Path, out: Path):
    sys.path.insert(0, str(repo))
    from sam2.build_sam import build_sam2

    model = build_sam2("configs/sam2.1/sam2.1_hiera_t.yaml", str(ckpt), device="cpu")
    model.eval()

    enc = SamEncoder(model).eval()
    image = torch.randn(1, 3, 1024, 1024)
    with torch.no_grad():
        want = enc(image)
    enc_path = out / "sam2.1_hiera_tiny_encoder.onnx"
    torch.onnx.export(enc, (image,), str(enc_path), opset_version=OPSET, dynamo=False,
                      input_names=["image"],
                      output_names=["image_embed", "high_res_0", "high_res_1"])
    print(f"wrote {enc_path}")
    sess = ort.InferenceSession(str(enc_path), providers=["CPUExecutionProvider"])
    check("encoder", sess, {"image": image}, want, atol=5e-3)

    dec = SamDecoder(model).eval()
    feeds = {
        "image_embed": want[0],
        "high_res_0": want[1],
        "high_res_1": want[2],
        "point_coords": torch.tensor([[[500.0, 400.0], [100.0, 900.0]]]),
        "point_labels": torch.tensor([[1.0, 0.0]]),
        "mask_input": torch.randn(1, 1, 256, 256),
        "has_mask_input": torch.tensor([1.0]),
    }
    with torch.no_grad():
        dwant = dec(*feeds.values())
    dec_path = out / "sam2.1_hiera_tiny_decoder.onnx"
    torch.onnx.export(dec, tuple(feeds.values()), str(dec_path), opset_version=OPSET, dynamo=False,
                      input_names=list(feeds.keys()), output_names=["masks", "iou"],
                      dynamic_axes={"point_coords": {1: "n"}, "point_labels": {1: "n"}})
    print(f"wrote {dec_path}")
    sess = ort.InferenceSession(str(dec_path), providers=["CPUExecutionProvider"])
    check("decoder", sess, feeds, dwant, atol=5e-3)
    # A different prompt count through the same graph (dynamic axis).
    feeds["point_coords"] = torch.tensor([[[10.0, 20.0], [300.0, 300.0], [800.0, 800.0]]])
    feeds["point_labels"] = torch.tensor([[2.0, 3.0, 1.0]])
    feeds["has_mask_input"] = torch.tensor([0.0])
    with torch.no_grad():
        dwant = dec(*feeds.values())
    check("decoder (box + point, no mask)", sess, feeds, dwant, atol=5e-3)


# --------------------------------------------------------------------------
# PMRID
# --------------------------------------------------------------------------

def export_pmrid(repo: Path, out: Path):
    sys.path.insert(0, str(repo))
    from models.net_torch import Network

    net = Network()
    # A torch.save state dict; weights_only refuses anything but tensors.
    states = torch.load(repo / "models" / "torch_pretrained.ckp", map_location="cpu", weights_only=True)
    if "state_dict" in states:
        states = states["state_dict"]
    net.load_state_dict(states)
    net.eval()

    x = torch.rand(1, 4, 256, 320) * 64.0
    with torch.no_grad():
        want = (net(x),)
    path = out / "pmrid.onnx"
    torch.onnx.export(net, (x,), str(path), opset_version=OPSET, dynamo=False,
                      input_names=["input"], output_names=["output"],
                      dynamic_axes={"input": {2: "h", 3: "w"}, "output": {2: "h", 3: "w"}})
    print(f"wrote {path}")
    sess = ort.InferenceSession(str(path), providers=["CPUExecutionProvider"])
    check("pmrid", sess, {"input": x}, want, atol=2e-3)
    x2 = torch.rand(1, 4, 160, 96) * 64.0
    with torch.no_grad():
        want2 = (net(x2),)
    check("pmrid (other size)", sess, {"input": x2}, want2, atol=2e-3)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--sam2-repo", type=Path)
    p.add_argument("--sam2-ckpt", type=Path)
    p.add_argument("--pmrid-repo", type=Path)
    p.add_argument("--out", type=Path, required=True)
    a = p.parse_args()
    a.out.mkdir(parents=True, exist_ok=True)
    torch.set_grad_enabled(False)
    if a.pmrid_repo:
        export_pmrid(a.pmrid_repo.resolve(), a.out)
    if a.sam2_repo:
        export_sam(a.sam2_repo.resolve(), a.sam2_ckpt.resolve(), a.out)

    manifest = {}
    for f in sorted(a.out.glob("*.onnx")):
        manifest[f.name] = {
            "sha256": hashlib.sha256(f.read_bytes()).hexdigest(),
            "bytes": f.stat().st_size,
        }
    (a.out / "manifest.json").write_text(json.dumps(manifest, indent=2))
    print(json.dumps(manifest, indent=2))


if __name__ == "__main__":
    main()
