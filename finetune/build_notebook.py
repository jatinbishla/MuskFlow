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
# MuskFlow Lite: fine-tune round 2

Teaches Qwen3.5-0.8B (MuskFlow's Lite model) to turn Lite's raw speech transcript into the text you meant, in three modes:
- **clean:** minimal edits
- **formal:** polished writing for email and Teams
- **prompt:** a concise instruction for Claude, ChatGPT or a coding agent

Round 2 also fixes what round 1 got wrong on real speech:
- **Phrase corrections:** "Wednesday at 3 pm, no, make that Thursday morning" now keeps only the corrected phrase.
- **Real words kept:** "I'm" and "like" are no longer deleted as if they were fillers.

**Start:** *Runtime → Change runtime type → T4 GPU*, then *Runtime → Run all*. Approve the Google Drive pop-up right away (first cell). After that it needs nothing from you for about 2 hours. Keep this tab in front and the laptop awake.

**If Colab disconnects:** click *Reconnect*, then *Run all* again. Everything is saved to Google Drive (`MyDrive/muskflow/work-r2/`) as it goes, so finished steps are skipped and training resumes from its last checkpoint.

| Step | Time on a T4 |
|---|---|
| 1. Connect Drive, install, build llama.cpp's quantizer | ~6 min |
| 2. Smoke test (every step on a tiny sample, so problems show up early) | ~6 min |
| 3. Reuse round 1's 2,513 messages and add 500 new ones (intros, real "I'm"/"like") | ~4 min |
| 4. Teacher writes 1,500 spoken-request → AI prompt pairs and 1,000 casual → formal pairs | ~25 min |
| 5. Make messy/clean pairs | <1 min |
| 6. Fine-tune | ~70–80 min |
| 7. Merge, check each mode on held-out pairs, convert to GGUF Q4_K_M | ~8 min |

The result is `MyDrive/muskflow/MuskFlow-Lite-0.8B-r2-Q4_K_M.gguf` (~520 MB). Put it in MuskFlow's `models/` folder; round 1's file can stay.
"""),
    md("## 1. Connect Drive and install"),
    code("""
import os, subprocess
os.environ["PYTORCH_CUDA_ALLOC_CONF"] = "expandable_segments:True"   # less fragmentation on a 15 GB T4
gpu = subprocess.run(["nvidia-smi", "-L"], capture_output=True, text=True).stdout
assert "GPU" in gpu, "No GPU: Runtime → Change runtime type → T4 GPU, then Run all again."
print(gpu)

def sh(cmd):
    # Runs a shell command and stops "Run all" if it fails, so later steps don't run on nothing.
    get_ipython().system(cmd)
    if get_ipython().user_ns.get("_exit_code", 0) != 0:
        raise RuntimeError(f"step failed: {cmd}")

from google.colab import drive
drive.mount('/content/drive')
OUT = '/content/drive/MyDrive/muskflow'
W = f'{OUT}/work-r2'   # every step saves here, so a disconnect loses nothing
!mkdir -p "{W}"
# Reuse round 1's clean messages (2,513) instead of writing them again.
![ -f "{W}/clean.jsonl" ] || [ ! -f "{OUT}/work/clean.jsonl" ] || cp "{OUT}/work/clean.jsonl" "{W}/clean.jsonl"
%cd /content

!pip install -q -U transformers peft accelerate num2words huggingface_hub safetensors
!pip uninstall -q -y torchao   # Colab's copy is too old for peft, and we don't need it
![ -d llama.cpp ] || git clone -q --depth 1 https://github.com/ggml-org/llama.cpp
!pip install -q ./llama.cpp/gguf-py sentencepiece
![ -x llama.cpp/build/bin/llama-quantize ] || (cmake -S llama.cpp -B llama.cpp/build -DLLAMA_CURL=OFF -DGGML_NATIVE=OFF > /dev/null && cmake --build llama.cpp/build --target llama-quantize -j 4 > /dev/null)
!ls llama.cpp/build/bin/llama-quantize && echo "ready"
"""),
    writefile("/content/lite_cleanup.py", (HERE / "lite_cleanup.py").read_text()),
    writefile("/content/heldout.txt", heldout_text()),
    md("## 2. Smoke test\nRuns every step on a tiny sample. Skipped once it has passed. If this cell fails, stop and send the error."),
    code("""
import os
if os.path.exists(f"{OUT}/smoke-r2-passed"):
    print("smoke test already passed")
else:
    sh("rm -rf /content/smoke")
    sh("python lite_cleanup.py gen --work /content/smoke --n 8 --batch 2")
    sh("python lite_cleanup.py styles --work /content/smoke --n-prompt 3 --n-formal 3 --batch 2")
    sh("python lite_cleanup.py pairs --work /content/smoke --versions 2")
    sh("python lite_cleanup.py train --work /content/smoke --batch 4 --max-steps 3")
    sh("python lite_cleanup.py export --work /content/smoke")
    sh("python lite_cleanup.py check --work /content/smoke --n 6")
    sh("python llama.cpp/convert_hf_to_gguf.py /content/smoke/merged --outtype f16 --outfile /content/smoke/test-f16.gguf > /content/smoke/convert.log 2>&1 || (tail -20 /content/smoke/convert.log; exit 1)")
    sh(f'test -s /content/smoke/test-f16.gguf && touch "{OUT}/smoke-r2-passed"')
    print("SMOKE TEST PASSED")
"""),
    md("## 3. Clean messages: round 1's, plus 500 new ones (resumes if interrupted)"),
    code('sh(f\'python lite_cleanup.py gen --work "{W}" --n 3000 --batch 32\')'),
    md("## 4. Teacher writes AI-prompt and formal pairs (resumes if interrupted)"),
    code('sh(f\'python lite_cleanup.py styles --work "{W}" --n-prompt 1500 --n-formal 1000 --batch 24\')'),
    md("## 5. Messy / clean pairs"),
    code("""
sh(f'python lite_cleanup.py pairs --work "{W}" --versions 2 --clean-limit 1800')
sh(f'head -n 4 "{W}/pairs.jsonl"; tail -n 4 "{W}/pairs.jsonl"')
"""),
    md("## 6. Fine-tune (checkpoints every 50 steps; resumes if interrupted)"),
    code('sh(f\'python lite_cleanup.py train --work "{W}" --batch 32 --epochs 1.5\')'),
    md("## 7. Merge, check, convert"),
    code("""
sh(f'python lite_cleanup.py export --work "{W}"')
sh(f'python lite_cleanup.py check --work "{W}" --n 300')
sh(f'python llama.cpp/convert_hf_to_gguf.py "{W}/merged" --outtype f16 --outfile /content/lite-f16.gguf')
sh(f'llama.cpp/build/bin/llama-quantize /content/lite-f16.gguf "{OUT}/MuskFlow-Lite-0.8B-r2-Q4_K_M.gguf" Q4_K_M')
sh(f'ls -lh "{OUT}"')
print("Done. Download MuskFlow-Lite-0.8B-r2-Q4_K_M.gguf from Google Drive → muskflow/ into MuskFlow's models/ folder.")
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
