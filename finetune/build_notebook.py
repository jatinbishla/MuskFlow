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
# MuskFlow Lite: fine-tune round 3

Teaches Qwen3.5-0.8B (MuskFlow's Lite model) to turn Lite's raw speech transcript into the text you meant, in four modes:
- **clean:** minimal edits
- **formal:** polished writing for email and Teams
- **prompt:** a concise instruction for Claude, ChatGPT or a coding agent
- **command:** Fn + Control. What's on screen plus a spoken instruction ("reply saying I'll send it Friday") → the text to type

Round 3 fixes what round 2 got wrong:
- **Lists:** "first… second… third" stays a numbered list in AI prompts.
- **Qualifiers kept:** words like "open source" are no longer dropped.
- **Questions stay questions:** "did you get the invoice?" no longer becomes "send the invoice".
- **Capitals:** names and days are written with capitals ("Friday", not "friday").

It also adds command-mode examples.

**Start:** *Runtime → Change runtime type → T4 GPU*, then *Runtime → Run all*. Approve the Google Drive pop-up right away (first cell). After that it needs nothing from you for about 3 hours. Keep this tab in front and the laptop awake.

**If Colab disconnects:** click *Reconnect*, then *Run all* again. Everything is saved to Google Drive (`MyDrive/muskflow/work-r3/`) as it goes, so finished steps are skipped and training resumes from its last checkpoint.

| Step | Time on a T4 |
|---|---|
| 1. Connect Drive, install, build llama.cpp's quantizer | ~6 min |
| 2. Smoke test (every step on a tiny sample) | ~8 min |
| 3. Reuse round 2's messages (nothing new to write) | <1 min |
| 4. Re-check round 2's AI-prompt/formal pairs with the stricter rules, and write replacements for the ones dropped | ~20–40 min |
| 5. Teacher writes 800 command-mode examples | ~40–60 min |
| 6. Make messy/clean pairs | <1 min |
| 7. Fine-tune | ~90–100 min |
| 8. Merge, check each mode on held-out pairs, convert to GGUF Q4_K_M | ~8 min |

The result is `MyDrive/muskflow/MuskFlow-Lite-0.8B-r3-Q4_K_M.gguf` (~520 MB). Put it in MuskFlow's `models/` folder.
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
W = f'{OUT}/work-r3'   # every step saves here, so a disconnect loses nothing
!mkdir -p "{W}"
# Reuse round 2's messages and style pairs instead of writing them again.
for f in ["clean.jsonl", "styles.jsonl"]:
    for prev in ["work-r2", "work"]:
        if not os.path.exists(f"{W}/{f}") and os.path.exists(f"{OUT}/{prev}/{f}"):
            sh(f'cp "{OUT}/{prev}/{f}" "{W}/{f}"')
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
if os.path.exists(f"{OUT}/smoke-r3-passed"):
    print("smoke test already passed")
else:
    sh("rm -rf /content/smoke")
    sh("python lite_cleanup.py gen --work /content/smoke --n 8 --batch 2")
    sh("python lite_cleanup.py styles --work /content/smoke --n-prompt 3 --n-formal 3 --batch 2")
    sh("python lite_cleanup.py command --work /content/smoke --n-command 3 --batch 2")
    sh("python lite_cleanup.py pairs --work /content/smoke --versions 2")
    sh("python lite_cleanup.py train --work /content/smoke --batch 4 --micro 4 --max-steps 3")
    sh("python lite_cleanup.py export --work /content/smoke")
    sh("python lite_cleanup.py check --work /content/smoke --n 6")
    sh("python llama.cpp/convert_hf_to_gguf.py /content/smoke/merged --outtype f16 --outfile /content/smoke/test-f16.gguf > /content/smoke/convert.log 2>&1 || (tail -20 /content/smoke/convert.log; exit 1)")
    sh(f'test -s /content/smoke/test-f16.gguf && touch "{OUT}/smoke-r3-passed"')
    print("SMOKE TEST PASSED")
"""),
    md("## 3. Clean messages (round 2's; skipped when already there)"),
    code('sh(f\'python lite_cleanup.py gen --work "{W}" --n 3000 --batch 32\')'),
    md("## 4. AI-prompt and formal pairs: re-check round 2's, top up (resumes if interrupted)"),
    code('sh(f\'python lite_cleanup.py styles --work "{W}" --n-prompt 1500 --n-formal 1000 --batch 24\')'),
    md("## 5. Command-mode examples (resumes if interrupted)"),
    code('sh(f\'python lite_cleanup.py command --work "{W}" --n-command 800 --batch 16\')'),
    md("## 6. Messy / clean pairs"),
    code("""
sh(f'python lite_cleanup.py pairs --work "{W}" --versions 2 --clean-limit 1500')
sh(f'head -n 4 "{W}/pairs.jsonl"; tail -n 4 "{W}/pairs.jsonl"')
"""),
    md("## 7. Fine-tune (checkpoints every 50 steps; resumes if interrupted)"),
    code('sh(f\'python lite_cleanup.py train --work "{W}" --batch 32 --micro 4 --epochs 1.5\')'),
    md("## 8. Merge, check, convert"),
    code("""
sh(f'python lite_cleanup.py export --work "{W}"')
sh(f'python lite_cleanup.py check --work "{W}" --n 300')
sh(f'python llama.cpp/convert_hf_to_gguf.py "{W}/merged" --outtype f16 --outfile /content/lite-f16.gguf')
sh(f'llama.cpp/build/bin/llama-quantize /content/lite-f16.gguf "{OUT}/MuskFlow-Lite-0.8B-r3-Q4_K_M.gguf" Q4_K_M')
sh(f'ls -lh "{OUT}"')
print("Done. Download MuskFlow-Lite-0.8B-r3-Q4_K_M.gguf from Google Drive → muskflow/ into MuskFlow's models/ folder.")
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
