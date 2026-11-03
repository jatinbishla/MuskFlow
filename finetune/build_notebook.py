"""Builds MuskFlow_Lite_finetune.ipynb: a self-contained Colab notebook that embeds
lite_cleanup.py and the held-out benchmark text, so it needs nothing from this repo.

    python3 build_notebook.py
"""

import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent


def heldout_text() -> str:
    files = sorted((ROOT / "bench/passages").glob("*.txt")) + sorted((ROOT / "bench/clips").glob("*.txt"))
    return "\n".join(f.read_text().strip().replace("\n", " ") for f in files)


def md(text):
    return {"cell_type": "markdown", "metadata": {}, "source": text.strip("\n").splitlines(keepends=True)}


def code(text):
    return {"cell_type": "code", "metadata": {}, "execution_count": None, "outputs": [],
            "source": text.strip("\n").splitlines(keepends=True)}


def writefile(name, body):
    return code(f"%%writefile {name}\n{body}")


cells = [
    md("""
# MuskFlow Lite: fine-tune the cleanup model

Teaches Qwen3.5-0.8B (MuskFlow's Lite cleanup model) one job: Lite's raw speech transcript in, the text you meant out.

**Before you start:** *Runtime → Change runtime type → T4 GPU* (free), then *Runtime → Run all*. About 1 hour; you can close the laptop lid only if Colab Pro's background execution is on, otherwise keep this tab open.

| Step | Time on a T4 |
|---|---|
| 1. Install + build llama.cpp's quantizer | ~6 min |
| 2. Smoke test (every step on a tiny sample, so problems show up early) | ~5 min |
| 3. Teacher writes 2,500 clean messages | ~15 min |
| 4. Make messy/clean pairs | <1 min |
| 5. Fine-tune | ~15–25 min |
| 6. Merge, check on held-out pairs, convert to GGUF Q4_K_M | ~8 min |
| 7. Save to Google Drive and download | ~2 min |

The result is one file, `MuskFlow-Lite-Cleanup-0.8B-Q4_K_M.gguf` (~530 MB). Put it in MuskFlow's `models/` folder.
"""),
    md("## 1. Install"),
    code("""
!nvidia-smi -L
!pip install -q -U transformers peft accelerate num2words huggingface_hub safetensors
!git clone -q --depth 1 https://github.com/ggml-org/llama.cpp
!pip install -q ./llama.cpp/gguf-py sentencepiece
!cmake -S llama.cpp -B llama.cpp/build -DLLAMA_CURL=OFF -DGGML_NATIVE=OFF > /dev/null && cmake --build llama.cpp/build --target llama-quantize -j 4 > /dev/null && echo "llama-quantize built"
"""),
    writefile("lite_cleanup.py", (HERE / "lite_cleanup.py").read_text()),
    writefile("heldout.txt", heldout_text()),
    md("## 2. Smoke test\nRuns every step on a tiny sample. If this cell fails, stop here and send the error."),
    code("""
!rm -rf smoke
!python lite_cleanup.py gen --work smoke --n 8 --batch 2
!python lite_cleanup.py pairs --work smoke --versions 3
!python lite_cleanup.py train --work smoke --batch 4 --max-steps 3
!python lite_cleanup.py export --work smoke
!python lite_cleanup.py check --work smoke --n 4
!python llama.cpp/convert_hf_to_gguf.py smoke/merged --outtype f16 --outfile smoke/test-f16.gguf > smoke/convert.log 2>&1 && echo "GGUF conversion OK" || tail -20 smoke/convert.log
"""),
    md("## 3. Teacher writes clean messages"),
    code("!python lite_cleanup.py gen --n 2500 --batch 32"),
    md("## 4. Messy / clean pairs"),
    code("""
!python lite_cleanup.py pairs --versions 2
!head -n 6 work/pairs.jsonl
"""),
    md("## 5. Fine-tune"),
    code("!python lite_cleanup.py train --batch 32 --epochs 2"),
    md("## 6. Merge, check, convert"),
    code("""
!python lite_cleanup.py export
!python lite_cleanup.py check --n 200
!python llama.cpp/convert_hf_to_gguf.py work/merged --outtype f16 --outfile work/lite-f16.gguf
!llama.cpp/build/bin/llama-quantize work/lite-f16.gguf MuskFlow-Lite-Cleanup-0.8B-Q4_K_M.gguf Q4_K_M
!ls -lh MuskFlow-Lite-Cleanup-0.8B-Q4_K_M.gguf
"""),
    md("## 7. Save\nCopies the model (and the LoRA adapter + training data, for the published repo) to Google Drive → `MyDrive/muskflow/`, then downloads the model."),
    code("""
from google.colab import drive, files
drive.mount('/content/drive')
!mkdir -p /content/drive/MyDrive/muskflow
!cp MuskFlow-Lite-Cleanup-0.8B-Q4_K_M.gguf /content/drive/MyDrive/muskflow/
!tar czf /content/drive/MyDrive/muskflow/lite-cleanup-lora-and-data.tgz work/lora work/clean.jsonl work/pairs.jsonl work/test.jsonl
!ls -lh /content/drive/MyDrive/muskflow
files.download('MuskFlow-Lite-Cleanup-0.8B-Q4_K_M.gguf')
"""),
]

nb = {
    "nbformat": 4,
    "nbformat_minor": 5,
    "metadata": {
        "accelerator": "GPU",
        "colab": {"gpuType": "T4", "provenance": []},
        "kernelspec": {"display_name": "Python 3", "name": "python3"},
        "language_info": {"name": "python"},
    },
    "cells": cells,
}
out = HERE / "MuskFlow_Lite_finetune.ipynb"
out.write_text(json.dumps(nb, indent=1))
print(f"wrote {out}")
