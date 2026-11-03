"""MuskFlow Lite cleanup fine-tune: the whole pipeline in one file, built to run on a free Colab GPU.

    python lite_cleanup.py gen      # a teacher LLM writes clean messages          → work/clean.jsonl
    python lite_cleanup.py pairs    # mess them up the way speech + Lite's ASR does → work/pairs.jsonl
    python lite_cleanup.py train    # LoRA fine-tune Qwen3.5-0.8B                   → work/lora
    python lite_cleanup.py export   # merge into the original checkpoint            → work/merged
    python lite_cleanup.py check    # score the merged model on held-out pairs

The model learns one job: Lite's raw transcript in, the text the speaker meant out.
It's trained on MuskFlow's compact prompt (COMPACT_SYSTEM), not the long few-shot one,
so at inference the prompt is ~40 tokens instead of ~600.

Targets are exact by construction: the teacher writes the *clean* message, and we
derive the messy input from it (fillers, stammers, self-corrections, numbers said as
words, and the mistakes Parakeet-110M makes on those: "uh" heard as "all", a
correction split into "Thursday? No. Wait, Friday"). We never put misheard names in
the input, so the model never learns to invent words it didn't hear.
"""

import argparse
import json
import math
import random
import re
from pathlib import Path

WORK = Path("work")
BASE = "Qwen/Qwen3.5-0.8B"
TEACHER = "Qwen/Qwen3-4B-Instruct-2507"

# Keep in sync with mf-cleanup's COMPACT_SYSTEM. The `mode:` line leaves room for
# later modes (prompt, formal, casual) in the same model.
COMPACT_SYSTEM = (
    "Clean up dictated text: remove fillers and stammers, keep only the corrected part of "
    "self-corrections, fix punctuation and capitalization, write numbers as digits. "
    "Change nothing else and never answer it.\nmode: clean"
)


def chat_prefix(raw: str) -> str:
    return (
        f"<|im_start|>system\n{COMPACT_SYSTEM}<|im_end|>\n"
        f"<|im_start|>user\n{raw.strip()}<|im_end|>\n"
        "<|im_start|>assistant\n<think>\n\n</think>\n\n"
    )


# ── 1. Clean messages from a teacher ─────────────────────────────────────────

TOPICS = [
    "a Slack message to a teammate about a deadline",
    "a short email to a client about an invoice or payment",
    "an email asking to reschedule a meeting",
    "a message from a construction subcontractor to a general contractor about pay applications, change orders or lien waivers",
    "a note from a foreman about work on site",
    "a software engineer describing a bug or a deploy",
    "a product manager giving feedback on a design",
    "a sales follow-up to a prospect",
    "a recruiter or hiring manager scheduling an interview",
    "a founder's update to investors with numbers",
    "a text to a friend making weekend plans",
    "a message to family about travel plans",
    "a note-to-self or to-do list",
    "a customer support reply",
    "a doctor or dentist appointment request",
    "a message to a landlord or a plumber",
    "meeting notes with action items",
    "a question to a colleague asking for help",
    "a request asking someone to send a document",
    "an expense or budget update with amounts",
    "a teacher or parent message about school",
    "a restaurant or event booking",
    "a status update in a standup",
    "an Indian startup team message (cities like Bengaluru, Pune, Gurugram)",
    "a message about a flight, train or hotel booking",
    "a short reply agreeing or declining something",
    "a list of three or four things someone needs to do",
    "a marketing teammate discussing a campaign",
    "a finance person discussing quarterly numbers",
    "a researcher describing an experiment result",
    "a prompt someone types to an AI assistant asking for help with code or writing",
]
NAMES = (
    "Rohan Ananya Vikram Priyanka Arjun Kavya Siddharth Meera Aditya Ishita Karan Neha Rahul Pooja "
    "Sanjay Deepika Varun Aisha Imran Fatima Tom Emily James Olivia Michael Sophie Grace Lucas "
    "Chloe Ethan Hannah Noah Zoe Liam Ava Mateo Sofia Diego Yuki Kenji Mei Wei Amara Kwame "
    "Chidi Leila Omar Sven Ingrid Pierre Camille Marco Giulia Olga Dmitri Shreya Harpreet Manoj "
    "Lakshmi Suresh Tanvi Farhan Zara Dev Riya Aryan Simran Kabir Tara Raj Anjali"
).split()
COMPANIES = "Acme Northwind Bluefin Larsen Brightline Zenith Kestrel Orbit Stripe Notion Zoho Infosys Bechtel Swiggy Freshworks".split()
MONTHS = "January February March April May June July August September October November December".split()
WEEKDAYS = "Monday Tuesday Wednesday Thursday Friday Saturday Sunday".split()

GEN_PROMPT = """Write {k} different messages that a person might dictate with a voice-typing app on their laptop. Theme: {topic}.

Rules:
- Each message is how they want it *typed*: correct punctuation and capitalization, no filler words, no emojis, no markdown.
- Lengths vary from 6 to 60 words. Mix single sentences and short paragraphs.
- Use concrete details where they fit. You may use any of these: names {names}; {details}. Not every message needs numbers.
- Write times, money, percentages and dates with digits, and small counts of things as words.
- Some messages are questions or requests to another person. Some are casual, some formal.
- Every message must be different in wording and structure.

Output one JSON object per line, exactly like {{"text": "..."}}, and nothing else."""


def random_details() -> str:
    amount = random.choice([random.randint(5, 99), random.randint(100, 999), random.randint(1000, 99999), random.randint(100000, 9999999)])
    money = random.choice(["$", "$", "₹", "€", "£"]) + f"{amount:,}"
    hour = f"{random.randint(1, 12)}{random.choice(['', ':15', ':30', ':45'])} {random.choice(['am', 'pm'])}"
    day = random.choice(WEEKDAYS + ["tomorrow", "next week"])
    month = random.choice(MONTHS)
    date = random.choice([f"{month} {random.randint(1, 28)}", f"the {random.randint(4, 20)}th of {month}"])
    pct = f"{random.choice([random.randint(1, 99), round(random.uniform(0.5, 20), 1)])}%"
    picks = [f"the amount {money}", f"the time {hour}", f"the day {day}", f"the date {date}",
             f"the percentage {pct}", f"the company {random.choice(COMPANIES)}", f"the number {random.randint(2, 500)}"]
    random.shuffle(picks)
    return ", ".join(picks[: random.randint(2, 5)])


def held_out_ngrams(path: Path, n: int = 5) -> set:
    if not path.exists():
        return set()
    words = []
    grams = set()
    for line in path.read_text().splitlines():
        words = re.findall(r"[a-z0-9']+", line.lower())
        grams |= {tuple(words[i:i + n]) for i in range(len(words) - n + 1)}
    return grams


def leaks(text: str, grams: set, n: int = 5) -> bool:
    words = re.findall(r"[a-z0-9']+", text.lower())
    return any(tuple(words[i:i + n]) in grams for i in range(len(words) - n + 1))


def cmd_gen(args):
    import torch
    from transformers import AutoModelForCausalLM, AutoTokenizer

    out = WORK / "clean.jsonl"
    seen = {json.loads(l)["text"] for l in out.read_text().splitlines()} if out.exists() else set()
    if len(seen) >= args.n:
        print(f"{len(seen)} messages already written")
        return
    random.seed(args.seed + len(seen))
    grams = held_out_ngrams(Path(args.heldout))
    tok = AutoTokenizer.from_pretrained(TEACHER, padding_side="left")
    model = AutoModelForCausalLM.from_pretrained(TEACHER, dtype=torch.float16, device_map="cuda")
    dropped = 0
    with out.open("a") as f:
        while len(seen) < args.n:
            prompts = [
                tok.apply_chat_template(
                    [{"role": "user", "content": GEN_PROMPT.format(
                        k=4, topic=random.choice(TOPICS), names=", ".join(random.sample(NAMES, 5)), details=random_details())}],
                    tokenize=False, add_generation_prompt=True)
                for _ in range(args.batch)
            ]
            enc = tok(prompts, return_tensors="pt", padding=True).to("cuda")
            with torch.no_grad():
                gen = model.generate(**enc, max_new_tokens=450, do_sample=True, temperature=0.9, top_p=0.95)
            for text in tok.batch_decode(gen[:, enc.input_ids.shape[1]:], skip_special_tokens=True):
                for line in text.splitlines():
                    try:
                        t = json.loads(line.strip())["text"].strip().replace("’", "'")
                    except (ValueError, KeyError, TypeError):
                        continue
                    if not 4 <= len(t.split()) <= 80 or t in seen or any(c in t for c in "*#_~|<>@"):
                        continue
                    if leaks(t, grams):
                        dropped += 1
                        continue
                    seen.add(t)
                    f.write(json.dumps({"text": t}) + "\n")
            f.flush()
            print(f"{len(seen)}/{args.n} messages ({dropped} dropped: overlap with the benchmark)", flush=True)


# ── 2. Messy speech, as Lite's ASR would transcribe it ───────────────────────

FILLERS = ["um", "uh", "uh", "um", "er", "ah", "hmm"]
MISHEARD = {  # what Parakeet-110M actually wrote for these fillers in the benchmark
    "uh": ["a", "all", "are", "I'll", "oh", "ah", "up", "huh", "uh"],
    "um": ["I'm", "and", "um", "on", "him", "un", "them"],
    "er": ["or", "her", "a", "er"],
    "ah": ["oh", "a", "all", "Ah"],
    "hmm": ["hm", "mm", "him"],
}
RELATIVE = ["today", "tomorrow", "tonight", "next week", "this afternoon", "next month"]
INLINE_CUES = ["no wait", "no, sorry", "sorry", "I mean", "actually", "no, actually", "or rather", "no no"]
MAKE_THAT = ["No, actually, make that {v}.", "Actually, make that {v}.", "Sorry, make that {v}.",
             "Wait, no, {v}.", "No, sorry, I meant {v}."]
RESTART = ["Scratch that.", "No, scratch that.", "Sorry, let me start again.", "Wait, no.", "Actually, no."]
SLOT = re.compile(
    r"[$₹€£]\d[\d,]*(?:\.\d+)?|\b\d+(?:\.\d+)?%|\b\d{1,2}(?::\d{2})? ?(?:am|pm|AM|PM)\b"
    + "".join(r"|\b(?:" + "|".join(words) + r")\b" for words in (WEEKDAYS, MONTHS, RELATIVE))
)
CURRENCY = {"$": "dollars", "₹": "rupees", "€": "euros", "£": "pounds"}


def distractor(v: str) -> str:
    for group in (WEEKDAYS, MONTHS, RELATIVE):
        if v in group:
            return random.choice([x for x in group if x != v])
    m = re.search(r"\d[\d,]*(?:\.\d+)?", v)
    if not m:
        return v
    if re.search(r"am|pm", v, re.I):
        h = int(m.group().split(":")[0])
        return re.sub(r"^\d{1,2}", str((h + random.choice([1, 2, 3, 10])) % 12 or 12), v)
    n = float(m.group().replace(",", ""))
    new = n * random.choice([0.5, 0.8, 1.25, 1.5, 2]) + random.choice([0, 5, 10])
    new = round(new, 1) if "." in m.group() else int(new)
    if new == n:
        new = n + 1
    s = f"{new:,}" if "," in m.group() else str(new)
    return v[:m.start()] + s + v[m.end():]


def add_correction(text: str):
    slots = list(SLOT.finditer(text))
    if not slots:
        return None
    m = random.choice(slots)
    v, wrong = m.group(), distractor(m.group())
    if wrong == v:
        return None
    style = random.random()
    if style < 0.5:  # "Thursday, no wait, Friday"
        return text[:m.start()] + f"{wrong}, {random.choice(INLINE_CUES)}, {v}" + text[m.end():]
    pos = 0
    for s in re.findall(r"[^.?!]+[.?!]*\s*", text):
        if pos <= m.start() < pos + len(s):
            said = s[:m.start() - pos] + wrong + s[m.end() - pos:]
            if style < 0.8:  # "...Wednesday. No, actually, make that Thursday."
                new = said.rstrip() + " " + random.choice(MAKE_THAT).format(v=v) + " "
            else:  # "We ship Friday. Scratch that. We ship Monday."
                new = said.rstrip() + " " + random.choice(RESTART) + " " + s
            return text[:pos] + new + text[pos + len(s):]
        pos += len(s)
    return None


def add_disfluencies(text: str) -> str:
    out = []
    rate = random.uniform(0.03, 0.18)  # some speakers are tidy, some aren't
    for w in text.split():
        r = random.random()
        if r < rate:
            f = random.choice(FILLERS)
            out.append(f + "," if random.random() < 0.4 else f)
        elif r < rate + 0.03 and len(w) <= 4 and w.isalpha():
            out.append(w)  # stammer: "the the"
        out.append(w)
    return " ".join(out)


def say_number(tok: str) -> str:
    """'$12,400' → 'twelve thousand four hundred dollars', '15th' → 'fifteenth', '3:30' → 'three thirty'."""
    from num2words import num2words

    def words(n, to="cardinal"):
        return num2words(n, to=to).replace(",", "").replace(" and ", " ").replace("-", " ")

    m = re.fullmatch(r"([$₹€£]?)(\d[\d,]*)(\.\d+)?(%?)(st|nd|rd|th)?", tok)
    if not m:
        m2 = re.fullmatch(r"(\d{1,2}):(\d{2})", tok)
        if m2:
            mins = int(m2.group(2))
            return words(int(m2.group(1))) + ("" if mins == 0 else " " + ("oh " if mins < 10 else "") + words(mins))
        return tok
    cur, num, dec, pct, ordinal = m.groups()
    n = int(num.replace(",", ""))
    if ordinal:
        return words(n, "ordinal")
    if 1900 <= n <= 2099 and not cur and not pct and not dec and "," not in num:
        return words(n, "year")
    s = words(n)
    if dec:
        s += " point " + " ".join(words(int(d)) for d in dec[1:])
    if pct:
        s += " percent"
    if cur:
        s += " " + CURRENCY[cur]
    return s


def asr_noise(spoken: str) -> str:
    """What Parakeet-110M tends to write for a messy utterance."""
    out = []
    numbers_as_words = random.random() < 0.7
    for w in spoken.split():
        core = w.strip(",.?!").lower()
        trail = w[len(w.rstrip(",.?!")):]
        if core in MISHEARD:
            r = random.random()
            if r < 0.15:
                continue  # dropped
            word = random.choice(MISHEARD[core]) if r < 0.65 else core
            out.append(word + random.choice([trail, "", ",", "."]))
            continue
        body = w.rstrip(",.?!")
        if numbers_as_words and re.search(r"\d", body):
            lead = re.match(r"^[^\w$₹€£]*", body).group()
            out.append(say_number(body[len(lead):]) + trail)
            continue
        out.append(w)
    text = " ".join(out)
    text = re.sub(r"\b(\d+|\w+) (am|pm)\b", lambda m: m.group(1) + " " + random.choice([m.group(2), m.group(2).upper()]), text)
    # Corrections often come out split into sentences: "Thursday? No. Wait, Friday".
    text = re.sub(r", (no|No)(,? )(wait|actually|sorry)", lambda m: random.choice(
        [m.group(0), f"? No. {m.group(3).capitalize()}", f". No, {m.group(3)}", f" no {m.group(3)}"]), text)
    # Punctuation is the ASR's guess: drop some commas, split some clauses.
    toks = text.split()
    for i, t in enumerate(toks):
        if t.endswith(",") and random.random() < 0.3:
            toks[i] = t[:-1]
        elif t.endswith(",") and random.random() < 0.05 and i + 1 < len(toks):
            toks[i] = t[:-1] + "."
            toks[i + 1] = toks[i + 1][:1].upper() + toks[i + 1][1:]
    text = " ".join(toks)
    return text[:1].upper() + text[1:] if text else text


def cmd_pairs(args):
    random.seed(args.seed)
    cleans = [json.loads(l)["text"] for l in (WORK / "clean.jsonl").read_text().splitlines() if l.strip()]
    n = 0
    with (WORK / "pairs.jsonl").open("w") as f:
        for clean in cleans:
            for k in range(args.versions):
                if k == 0 and random.random() < 0.08:
                    heard = clean  # already clean: leave it alone
                else:
                    spoken = clean
                    if random.random() < 0.45:
                        spoken = add_correction(spoken) or spoken
                    if random.random() < 0.85:
                        spoken = add_disfluencies(spoken)
                    heard = asr_noise(spoken)
                f.write(json.dumps({"input": heard, "target": clean}) + "\n")
                n += 1
    print(f"{n} pairs from {len(cleans)} messages")


# ── 3. LoRA fine-tune ────────────────────────────────────────────────────────

TARGETS = r".*language_model\.layers\.\d+\.(self_attn\.(q|k|v|o)_proj|linear_attn\.(in_proj_qkv|in_proj_z|out_proj)|mlp\.(gate|up|down)_proj)"


def split(pairs, seed):
    targets = sorted({p["target"] for p in pairs})
    random.Random(seed).shuffle(targets)
    # 4% held out (at least 20), but never more than a quarter: the smoke test has only a handful.
    test = set(targets[: min(max(20, len(targets) // 25), max(1, len(targets) // 4))])
    return [p for p in pairs if p["target"] not in test], [p for p in pairs if p["target"] in test]


def load_base(dtype):
    import torch
    from transformers import AutoModelForImageTextToText

    return AutoModelForImageTextToText.from_pretrained(BASE, dtype=dtype, device_map="cuda")


def cmd_train(args):
    import torch
    from peft import LoraConfig, get_peft_model
    from transformers import AutoTokenizer

    if (WORK / "lora" / "adapter_model.safetensors").exists():
        print(f"{WORK / 'lora'} already trained; delete it to train again")
        return
    bf16 = torch.cuda.is_bf16_supported()
    pairs = [json.loads(l) for l in (WORK / "pairs.jsonl").read_text().splitlines() if l.strip()]
    train, test = split(pairs, args.seed)
    (WORK / "test.jsonl").write_text("".join(json.dumps(p) + "\n" for p in test))
    tok = AutoTokenizer.from_pretrained(BASE)

    def encode(p):
        prompt = tok(chat_prefix(p["input"]), add_special_tokens=False).input_ids
        answer = tok(p["target"] + "<|im_end|>", add_special_tokens=False).input_ids
        return prompt + answer, len(prompt)

    data = [encode(p) for p in train]
    print(f"{len(train)} train / {len(test)} test pairs · longest {max(len(d) for d, _ in data)} tokens")

    # fp32 weights + fp16 autocast on T4 (no bf16); bf16 throughout on newer GPUs.
    model = load_base(torch.bfloat16 if bf16 else torch.float32)
    model.config.use_cache = False
    model = get_peft_model(model, LoraConfig(r=args.rank, lora_alpha=args.rank * 2, lora_dropout=0.05, target_modules=TARGETS))
    model.print_trainable_parameters()
    model.gradient_checkpointing_enable(gradient_checkpointing_kwargs={"use_reentrant": False})
    model.enable_input_require_grads()

    steps_per_epoch = math.ceil(len(data) / args.batch)
    total = int(steps_per_epoch * args.epochs)
    if args.max_steps:
        total = min(total, args.max_steps)
    opt = torch.optim.AdamW([p for p in model.parameters() if p.requires_grad], lr=args.lr, weight_decay=0.01)
    warmup = max(10, total // 20)
    sched = torch.optim.lr_scheduler.LambdaLR(
        opt, lambda s: min(1, (s + 1) / warmup) * (0.1 + 0.9 * 0.5 * (1 + math.cos(math.pi * min(1, s / total)))))
    scaler = torch.amp.GradScaler(enabled=not bf16)
    amp = torch.bfloat16 if bf16 else torch.float16

    trainable = [p for p in model.parameters() if p.requires_grad]
    ckpt = WORK / "checkpoint.pt"

    def save_checkpoint():
        tmp = ckpt.with_suffix(".tmp")
        torch.save({"step": step, "weights": [p.detach().cpu() for p in trainable], "opt": opt.state_dict(),
                    "sched": sched.state_dict(), "scaler": scaler.state_dict()}, tmp)
        tmp.replace(ckpt)  # atomic, so a disconnect mid-save can't corrupt it

    rng = random.Random(args.seed)
    model.train()
    step = 0
    if ckpt.exists():  # Colab disconnected: carry on where we stopped
        c = torch.load(ckpt, weights_only=False)
        with torch.no_grad():
            for p, w in zip(trainable, c["weights"]):
                p.copy_(w.to(p.device))
        opt.load_state_dict(c["opt"])
        sched.load_state_dict(c["sched"])
        scaler.load_state_dict(c["scaler"])
        step = c["step"]
        rng = random.Random(args.seed + step)
        print(f"resuming from step {step}/{total}")
    import time
    t0, start = time.time(), step
    while step < total:
        # Length-bucketed batches: less padding, faster.
        order = sorted(range(len(data)), key=lambda i: len(data[i][0]) + rng.random() * 8)
        batches = [order[i:i + args.batch] for i in range(0, len(order), args.batch)]
        rng.shuffle(batches)
        for b in batches:
            if step >= total:
                break
            width = max(len(data[i][0]) for i in b)
            ids = torch.full((len(b), width), tok.pad_token_id or 0)
            labels = torch.full((len(b), width), -100)
            mask = torch.zeros((len(b), width), dtype=torch.long)
            for r, i in enumerate(b):
                seq, plen = data[i]
                ids[r, :len(seq)] = torch.tensor(seq)
                labels[r, plen:len(seq)] = torch.tensor(seq[plen:])
                mask[r, :len(seq)] = 1
            with torch.autocast("cuda", dtype=amp):
                loss = model(input_ids=ids.cuda(), attention_mask=mask.cuda(), labels=labels.cuda()).loss
            scaler.scale(loss).backward()
            scaler.unscale_(opt)
            torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0)
            scaler.step(opt)
            scaler.update()
            opt.zero_grad(set_to_none=True)
            sched.step()
            step += 1
            if step % 50 == 0:
                save_checkpoint()
            if step % 20 == 0 or step == total:
                el = time.time() - t0
                print(f"step {step}/{total}  loss {loss.item():.4f}  {el / 60:.1f} min elapsed, ~{el / max(1, step - start) * (total - step) / 60:.1f} min left", flush=True)
    model.save_pretrained(str(WORK / "lora"))
    print("saved", WORK / "lora")


# ── 4. Merge into the original checkpoint ────────────────────────────────────

def cmd_export(args):
    """Skips if already done. Adds the LoRA deltas to the original safetensors, keeping every other tensor (vision,
    MTP) and file untouched, so llama.cpp's converter sees exactly the official layout."""
    import shutil

    import torch
    from huggingface_hub import snapshot_download
    from peft import PeftModel
    from safetensors.torch import load_file, save_file

    dst = WORK / "merged"
    if (dst / "DONE").exists():
        print(f"{dst} already exported")
        return
    src = Path(snapshot_download(BASE))
    model = PeftModel.from_pretrained(load_base(torch.float32), str(WORK / "lora")).merge_and_unload()
    merged = {k: v for k, v in model.state_dict().items() if re.match(TARGETS, k)}
    dst.mkdir(exist_ok=True)
    for f in src.iterdir():
        if not f.name.endswith(".safetensors"):
            shutil.copy(f, dst / f.name)
    replaced = 0
    for f in sorted(src.glob("*.safetensors")):
        tensors = load_file(f)
        for k in tensors:
            if k in merged:
                tensors[k] = merged[k].to(tensors[k].dtype).contiguous()
                replaced += 1
        save_file(tensors, dst / f.name, metadata={"format": "pt"})
    assert replaced > 0, "no LoRA weights matched the checkpoint's tensor names"
    (dst / "DONE").write_text(str(replaced))
    print(f"merged {replaced} weight matrices into {dst}")


# ── 5. Held-out check ────────────────────────────────────────────────────────

def wer(ref: str, hyp: str) -> float:
    r = re.findall(r"[a-z0-9$%']+", ref.lower())
    h = re.findall(r"[a-z0-9$%']+", hyp.lower())
    d = list(range(len(h) + 1))
    for i, a in enumerate(r, 1):
        prev, d[0] = d[0], i
        for j, b in enumerate(h, 1):
            prev, d[j] = d[j], min(d[j] + 1, d[j - 1] + 1, prev + (a != b))
    return d[len(h)] / max(1, len(r))


def cmd_check(args):
    import torch
    from transformers import AutoModelForImageTextToText, AutoTokenizer

    tok = AutoTokenizer.from_pretrained(str(WORK / "merged"), padding_side="left")
    model = AutoModelForImageTextToText.from_pretrained(str(WORK / "merged"), dtype=torch.float16, device_map="cuda").eval()
    test = [json.loads(l) for l in (WORK / "test.jsonl").read_text().splitlines()][: args.n]
    errs, words, exact = 0.0, 0, 0
    for i in range(0, len(test), 16):
        chunk = test[i:i + 16]
        enc = tok([chat_prefix(p["input"]) for p in chunk], return_tensors="pt", padding=True, add_special_tokens=False).to("cuda")
        with torch.no_grad():
            gen = model.generate(**enc, max_new_tokens=160, do_sample=False)
        for p, out in zip(chunk, tok.batch_decode(gen[:, enc.input_ids.shape[1]:], skip_special_tokens=True)):
            out = out.strip()
            n = len(p["target"].split())
            errs += wer(p["target"], out) * n
            words += n
            exact += out == p["target"]
            if i == 0:
                print(f"  in : {p['input']}\n  out: {out}\n  ref: {p['target']}\n")
    print(f"held-out: WER {100 * errs / words:.1f}% · exact match {exact}/{len(test)}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("step", choices=["gen", "pairs", "train", "export", "check"])
    ap.add_argument("--n", type=int, default=2500, help="gen: clean messages; check: test pairs")
    ap.add_argument("--batch", type=int, default=32, help="gen: prompts per batch; train: pairs per step")
    ap.add_argument("--versions", type=int, default=2, help="messy inputs per clean message")
    ap.add_argument("--heldout", default="heldout.txt")
    ap.add_argument("--rank", type=int, default=16)
    ap.add_argument("--lr", type=float, default=2e-4)
    ap.add_argument("--epochs", type=float, default=2.0)
    ap.add_argument("--seed", type=int, default=0)
    ap.add_argument("--max-steps", type=int, default=0, help="train: stop early (smoke test)")
    ap.add_argument("--work", default="work")
    args = ap.parse_args()
    global WORK
    WORK = Path(args.work)
    WORK.mkdir(exist_ok=True)
    {"gen": cmd_gen, "pairs": cmd_pairs, "train": cmd_train, "export": cmd_export, "check": cmd_check}[args.step](args)


if __name__ == "__main__":
    main()
