# AI model export

RawEditor's AI features run ONNX models through ONNX Runtime (`src/ml`).
Their upstream projects publish PyTorch checkpoints, so `export.py` converts
them once. The results are attached to the `models-v1` GitHub release and
listed, with their SHA-256s, in `src/ml/models.rs`. The app downloads them
only when the user asks.

| File | From | Licence |
|---|---|---|
| `sam2.1_hiera_tiny_encoder.onnx`, `sam2.1_hiera_tiny_decoder.onnx` | [facebookresearch/sam2](https://github.com/facebookresearch/sam2), checkpoint `sam2.1_hiera_tiny.pt` | Apache-2.0 (code and checkpoints) |
| `pmrid.onnx` | [MegEngine/PMRID](https://github.com/MegEngine/PMRID), `models/torch_pretrained.ckp` | Apache-2.0 |

The ONNX files are modified forms of those works: the graphs are
re-serialised, and the SAM decoder wrapper in `export.py` merges the prompt
encoder and mask decoder and returns all four mask tokens. The release
carries the Apache-2.0 licence text and a NOTICE that says so.

## Re-exporting

```sh
python -m venv venv
venv/bin/pip install --index-url https://download.pytorch.org/whl/cpu torch torchvision
venv/bin/pip install onnx onnxruntime onnxscript numpy hydra-core iopath pillow tqdm
git clone --depth 1 https://github.com/facebookresearch/sam2
git clone --depth 1 https://github.com/MegEngine/PMRID
curl -LO https://dl.fbaipublicfiles.com/segment_anything_2/092824/sam2.1_hiera_tiny.pt
venv/bin/python export.py --sam2-repo sam2 --sam2-ckpt sam2.1_hiera_tiny.pt \
                          --pmrid-repo PMRID --out dist
```

The script compares every graph with PyTorch through ONNX Runtime (it must
agree to about 1e-5) and writes `dist/manifest.json` with each file's size and
SHA-256. A new export means a new release tag and new records in
`src/ml/models.rs`. The PMRID cache key includes the model's hash, so cached
results from an older model are never reused.

To try freshly exported files without a release, point the app at them:
`RAWEDITOR_MODELS_DIR=dist cargo run --release`. The ignored tests
(`clicking_a_disc_selects_it`, `ai_denoise_vs_classical_nr`) use the same
variable.
