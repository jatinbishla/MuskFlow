"""MuskFlow Lite cleanup fine-tune: the whole pipeline in one file, built to run on a free Colab GPU.

    python lite_cleanup.py gen      # a teacher LLM writes clean messages          → work/clean.jsonl
    python lite_cleanup.py styles   # ...and spoken requests → AI prompts / formal  → work/styles.jsonl
    python lite_cleanup.py pairs    # mess them up the way speech + Lite's ASR does → work/pairs.jsonl
    python lite_cleanup.py train    # LoRA fine-tune Qwen3.5-0.8B                   → work/lora
    python lite_cleanup.py export   # merge into the original checkpoint            → work/merged
    python lite_cleanup.py check    # score the merged model on held-out pairs

The model learns Lite's raw transcript in, the text the speaker meant out, in three
modes: clean (minimal edits), formal (polished writing) and prompt (a concise
instruction for an AI assistant). It's trained on MuskFlow's compact prompts
(SYSTEMS), not the long few-shot ones, so at inference the prompt is ~40-60 tokens.

Round 2 (this version) fixes what round 1 got wrong on real speech: corrections
that replace a whole phrase ("Wednesday at 3 pm, no, make that Thursday morning"),
and deleting real words ("I'm", "like") because the noise model turned too many
fillers into them. It also adds the formal and prompt modes.

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

# Keep in sync with mf-cleanup's COMPACT_SYSTEM / COMPACT_FORMAL / COMPACT_PROMPT
# (`muskflow prompt --compact --style <mode>` prints them).
SYSTEMS = {
    "clean": (
        "Clean up dictated text: remove fillers and stammers, keep only the corrected part of "
        "self-corrections, fix punctuation and capitalization, write numbers as digits. "
        "Change nothing else and never answer it.\nmode: clean"
    ),
    "formal": (
        "Rewrite dictated text as polished, professional writing: remove fillers and stammers, "
        "keep only the corrected part of self-corrections, use complete sentences and proper "
        "punctuation, write numbers as digits. Keep every point, name and number, add nothing, "
        "and never answer it.\nmode: formal"
    ),
    "prompt": (
        "Rewrite a spoken request to an AI assistant as a clear, concise prompt: state what is "
        "wanted as an instruction, drop fillers, pleasantries and repetition, keep every "
        "requirement, name, number and identifier, and use a numbered list for several tasks. "
        "Add nothing, and never answer or solve it.\nmode: prompt"
    ),
}


def chat_prefix(raw: str, mode: str = "clean") -> str:
    return (
        f"<|im_start|>system\n{SYSTEMS[mode]}<|im_end|>\n"
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
    "someone introducing themselves (\"Hi, I'm …\") to a new client, investor or teammate",
    "a casual update that uses 'like' to mean roughly (\"like 90% done\", \"in like 10 minutes\")",
    "a message where the person says what they're doing (\"I'm on my way\", \"I'm working on it\")",
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


# ── 1b. Spoken requests → AI prompts, casual dictation → formal writing ──────

PROMPT_TOPICS = [
    "adding a feature to a web app", "debugging an error message", "refactoring messy code",
    "writing a SQL query", "a shell command they need", "asking an AI to write an email for them",
    "summarizing a long document", "researching a topic and comparing options",
    "analysing a spreadsheet of sales data", "writing marketing copy for a product launch",
    "planning a trip itinerary", "explaining a concept they're learning",
    "writing product requirements for a new screen", "giving a coding agent several tasks at once",
    "asking what's left to do on their project and what to work on next",
    "reviewing a pull request", "setting up a deployment or CI pipeline",
    "designing a database schema", "rewriting a paragraph to sound better",
    "preparing for a job interview", "translating a message", "fixing a failing test",
]
FORMAL_TOPICS = [
    "an email to a client about a delayed delivery", "an update to a manager on a project",
    "a Teams message asking a colleague for a file", "following up with a vendor about a payment",
    "scheduling an interview with a candidate", "a LinkedIn message to a recruiter",
    "a meeting recap with action items", "asking for approval on a budget",
    "a subcontractor emailing a general contractor about a pay application or lien waiver",
    "declining a meeting politely and proposing another time", "an email to a landlord about a repair",
]

STYLE_GEN = {
    "prompt": """Write {k} examples of someone speaking a request to an AI assistant (like ChatGPT, Claude or a coding agent) out loud, and the clear prompt they meant. Theme: {topic}.
For each example:
- "spoken": what they actually say out loud: natural, rambling speech with thinking out loud, pleasantries ("can you", "I was wondering", "so basically"), some repetition, and sometimes a change of mind. No filler sounds like um or uh. 20 to 90 words. Write numbers with digits. Names you may use: {names}.
- "out": the same request as a clear, concise instruction to the AI. Keep every requirement, constraint, name, number, file name and code identifier they said. Use a short intro line and a numbered list only when there are several separate tasks. Do not add anything they didn't ask for. Do not answer the request or write code.
Output one JSON object per line, exactly like {{"spoken": "...", "out": "..."}}, and nothing else.""",
    "formal": """Write {k} examples of someone dictating a work message out loud, and the polished version they would send. Theme: {topic}.
For each example:
- "spoken": casual, run-on speech as they would say it: loose grammar, repetition, "so basically", and sometimes a self-correction ("Tuesday, no, Wednesday"). No filler sounds like um or uh. 20 to 80 words. Write numbers with digits. Names you may use: {names}.
- "out": the same message as polished, professional writing: complete sentences, proper punctuation, short paragraphs if it is long. Keep every point, name, number and date, in order; after a self-correction keep only the corrected value. Do not add greetings, sign-offs, facts or promises they didn't say.
Output one JSON object per line, exactly like {{"spoken": "...", "out": "..."}}, and nothing else.""",
}

COMMON = set("""i i'm i'll i've i'd ok okay yes yeah no not now then please thanks thank hey hi hello what how why when where who which
should shall may might must let's let there here they he she his her their them us if as all some any every one first second third
next last maybe well actually also after before because the a an and or but so to of in on at for with by from is are was were be it
its this that we you me my our your can could would will do does just write create add make fix find list use check send keep include
explain summarize compare draft review update set build give show tell help run test""".split())


def names_in(text: str) -> set:
    words = text.split()
    out = set()
    for i, w in enumerate(words):
        core = w.strip(".,!?:;()\"'")
        if i == 0 or words[i - 1][-1:] in ".?!:" or re.match(r"^\d+[.)]$", words[i - 1]):
            continue
        # Days and months aren't names: a self-correction rightly drops "Tuesday".
        if len(core) > 1 and core[0].isupper() and core.lower() not in COMMON and core not in WEEKDAYS + MONTHS:
            out.add(core.lower())
    return out


def faithful(spoken: str, out: str) -> bool:
    """The rewrite keeps to what was said: no names or numbers that weren't spoken,
    every spoken name kept, no answer-like formatting, not much longer."""
    s, o = spoken.lower(), out.lower()
    if "```" in out or "**" in out or len(out.split()) > 1.2 * len(spoken.split()) + 5:
        return False
    nums = lambda t: set(re.findall(r"\d[\d,.:]*\d|\d", t.replace(",", "")))
    if not nums(o) <= nums(s):
        return False
    if any(n not in s for n in names_in(out)):
        return False
    return all(n in o for n in names_in(spoken))


def cmd_styles(args):
    import torch
    from transformers import AutoModelForCausalLM, AutoTokenizer

    out = WORK / "styles.jsonl"
    rows = [json.loads(l) for l in out.read_text().splitlines()] if out.exists() else []
    want = {"prompt": args.n_prompt, "formal": args.n_formal}
    have = {m: sum(r["mode"] == m for r in rows) for m in want}
    if all(have[m] >= want[m] for m in want):
        print(f"{have} style pairs already written")
        return
    random.seed(args.seed + len(rows))
    grams = held_out_ngrams(Path(args.heldout))
    tok = AutoTokenizer.from_pretrained(TEACHER, padding_side="left")
    model = AutoModelForCausalLM.from_pretrained(TEACHER, dtype=torch.float16, device_map="cuda")
    seen = {r["spoken"] for r in rows}
    rejected = 0
    with out.open("a") as f:
        while any(have[m] < want[m] for m in want):
            modes = [m for m in want if have[m] < want[m]]
            batch = [random.choice(modes) for _ in range(args.batch)]
            prompts = [
                tok.apply_chat_template(
                    [{"role": "user", "content": STYLE_GEN[m].format(
                        k=3, topic=random.choice(PROMPT_TOPICS if m == "prompt" else FORMAL_TOPICS),
                        names=", ".join(random.sample(NAMES, 4)))}],
                    tokenize=False, add_generation_prompt=True)
                for m in batch
            ]
            enc = tok(prompts, return_tensors="pt", padding=True).to("cuda")
            with torch.no_grad():
                gen = model.generate(**enc, max_new_tokens=900, do_sample=True, temperature=0.85, top_p=0.95)
            for mode, text in zip(batch, tok.batch_decode(gen[:, enc.input_ids.shape[1]:], skip_special_tokens=True)):
                for line in text.splitlines():
                    try:
                        r = json.loads(line.strip())
                        spoken, target = r["spoken"].strip().replace("’", "'"), r["out"].strip().replace("’", "'")
                    except (ValueError, KeyError, TypeError, AttributeError):
                        continue
                    if spoken in seen or not 8 <= len(spoken.split()) <= 120 or leaks(spoken, grams):
                        continue
                    if not faithful(spoken, target):
                        rejected += 1
                        continue
                    seen.add(spoken)
                    have[mode] += 1
                    f.write(json.dumps({"mode": mode, "spoken": spoken, "out": target}) + "\n")
            f.flush()
            print(f"{have} (rejected {rejected} unfaithful)", flush=True)


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


def sentences(text: str) -> list:
    """Splits into sentences, without breaking "9.6%" or "$4.50" at the decimal point."""
    return re.findall(r"(?:[^.?!]|(?<=\d)\.(?=\d))+[.?!]*\s*", text)


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
    for s in sentences(text):
        if pos <= m.start() < pos + len(s):
            said = s[:m.start() - pos] + wrong + s[m.end() - pos:]
            if style < 0.8:  # "...Wednesday. No, actually, make that Thursday."
                new = said.rstrip() + " " + random.choice(MAKE_THAT).format(v=v) + " "
            else:  # "We ship Friday. Scratch that. We ship Monday."
                new = said.rstrip() + " " + random.choice(RESTART) + " " + s
            return text[:pos] + new + text[pos + len(s):]
        pos += len(s)
    return None


PARTS = ["morning", "afternoon", "evening", "night"]
PHRASE = re.compile(
    r"\b(?:" + "|".join(WEEKDAYS + ["today", "tomorrow"]) + r")"
    r"(?: (?:" + "|".join(PARTS) + r"))?"
    r"(?: at \d{1,2}(?::\d{2})? ?(?:am|pm|AM|PM))?"
)


def other_phrase(phrase: str) -> str:
    """A different day/time phrase, often with a different shape ("Wednesday at 3 pm" vs
    "Thursday morning"), so the model learns a correction replaces the whole phrase."""
    for _ in range(10):
        day = random.choice(WEEKDAYS + ["tomorrow", "today"])
        tail = random.choice(["", " " + random.choice(PARTS),
                              f" at {random.randint(1, 12)}{random.choice(['', ':30'])} {random.choice(['am', 'pm'])}"])
        if (day + tail).lower() != phrase.lower():
            return day + tail
    return phrase


def add_phrase_correction(text: str):
    """Corrects a whole day/time phrase, as in "Wednesday at 3 pm. No, actually, make that
    Thursday morning." Returns None if the text has no such phrase."""
    phrases = [m for m in PHRASE.finditer(text) if m.group()]
    if not phrases:
        return None
    m = random.choice(phrases)
    v, wrong = m.group(), other_phrase(m.group())
    if random.random() < 0.5:
        return text[:m.start()] + f"{wrong}, {random.choice(INLINE_CUES)}, {v}" + text[m.end():]
    pos = 0
    for s in sentences(text):
        if pos <= m.start() < pos + len(s):
            said = s[:m.start() - pos] + wrong + s[m.end() - pos:]
            return text[:pos] + said.rstrip() + " " + random.choice(MAKE_THAT).format(v=v) + " " + text[pos + len(s):]
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
            # Round 1 misheard 50% of fillers as real words ("I'm", "and"), and the model
            # learned to delete real ones. Now 25%; the rest stay recognisable fillers.
            word = random.choice(MISHEARD[core]) if r < 0.40 else core
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
    if args.clean_limit and len(cleans) > args.clean_limit:
        cleans = random.sample(cleans, args.clean_limit)
    styles_file = WORK / "styles.jsonl"
    styles = [json.loads(l) for l in styles_file.read_text().splitlines()] if styles_file.exists() else []
    counts = {"clean": 0, "formal": 0, "prompt": 0}
    with (WORK / "pairs.jsonl").open("w") as f:
        for clean in cleans:
            for k in range(args.versions):
                if k == 0 and random.random() < 0.08:
                    heard = clean  # already clean: leave it alone
                else:
                    spoken = clean
                    r = random.random()
                    if r < 0.25:
                        spoken = add_phrase_correction(spoken) or add_correction(spoken) or spoken
                    elif r < 0.45:
                        spoken = add_correction(spoken) or spoken
                    if random.random() < 0.85:
                        spoken = add_disfluencies(spoken)
                    heard = asr_noise(spoken)
                f.write(json.dumps({"mode": "clean", "input": heard, "target": clean}) + "\n")
                counts["clean"] += 1
        for row in styles:
            spoken = row["spoken"]
            if random.random() < 0.85:
                spoken = add_disfluencies(spoken)
            f.write(json.dumps({"mode": row["mode"], "input": asr_noise(spoken), "target": row["out"]}) + "\n")
            counts[row["mode"]] += 1
    print(f"pairs: {counts}")


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
        prompt = tok(chat_prefix(p["input"], p.get("mode", "clean")), add_special_tokens=False).input_ids
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
            # Micro-batches add up to one step: a 248k-word vocabulary makes the logits huge,
            # so 32 examples at once don't fit a T4's 15 GB.
            for m in range(0, len(b), args.micro):
                mb = b[m:m + args.micro]
                width = max(len(data[i][0]) for i in mb)
                ids = torch.full((len(mb), width), tok.pad_token_id or 0)
                labels = torch.full((len(mb), width), -100)
                mask = torch.zeros((len(mb), width), dtype=torch.long)
                for r, i in enumerate(mb):
                    seq, plen = data[i]
                    ids[r, :len(seq)] = torch.tensor(seq)
                    labels[r, plen:len(seq)] = torch.tensor(seq[plen:])
                    mask[r, :len(seq)] = 1
                with torch.autocast("cuda", dtype=amp):
                    loss = model(input_ids=ids.cuda(), attention_mask=mask.cuda(), labels=labels.cuda()).loss
                scaler.scale(loss * len(mb) / len(b)).backward()
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
    by_mode = {}
    for i in range(0, len(test), 16):
        chunk = test[i:i + 16]
        enc = tok([chat_prefix(p["input"], p.get("mode", "clean")) for p in chunk], return_tensors="pt", padding=True, add_special_tokens=False).to("cuda")
        with torch.no_grad():
            gen = model.generate(**enc, max_new_tokens=160, do_sample=False)
        for p, out in zip(chunk, tok.batch_decode(gen[:, enc.input_ids.shape[1]:], skip_special_tokens=True)):
            out = out.strip()
            n = len(p["target"].split())
            errs += wer(p["target"], out) * n
            words += n
            exact += out == p["target"]
            m = by_mode.setdefault(p.get("mode", "clean"), {"errs": 0.0, "words": 0, "n": 0, "faithful": 0, "shown": 0})
            m["errs"] += wer(p["target"], out) * n
            m["words"] += n
            m["n"] += 1
            m["faithful"] += faithful(p["input"], out)
            if m["shown"] < 3:
                m["shown"] += 1
                print(f"  [{p.get('mode', 'clean')}]\n  in : {p['input']}\n  out: {out}\n  ref: {p['target']}\n")
    print(f"held-out: WER {100 * errs / words:.1f}% · exact match {exact}/{len(test)}")
    for mode, m in by_mode.items():
        print(f"  {mode:<7} WER vs reference {100 * m['errs'] / max(1, m['words']):.1f}% · "
              f"kept names/numbers {m['faithful']}/{m['n']} ({m['n']} pairs)")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("step", choices=["gen", "styles", "pairs", "train", "export", "check"])
    ap.add_argument("--n", type=int, default=2500, help="gen: clean messages; check: test pairs")
    ap.add_argument("--batch", type=int, default=32, help="gen: prompts per batch; train: pairs per step")
    ap.add_argument("--versions", type=int, default=2, help="messy inputs per clean message")
    ap.add_argument("--heldout", default="heldout.txt")
    ap.add_argument("--rank", type=int, default=16)
    ap.add_argument("--lr", type=float, default=2e-4)
    ap.add_argument("--epochs", type=float, default=2.0)
    ap.add_argument("--seed", type=int, default=0)
    ap.add_argument("--micro", type=int, default=8, help="train: examples per forward pass (memory)")
    ap.add_argument("--n-prompt", type=int, default=1500, help="styles: AI-prompt pairs")
    ap.add_argument("--n-formal", type=int, default=1000, help="styles: formal pairs")
    ap.add_argument("--clean-limit", type=int, default=0, help="pairs: use at most this many clean messages")
    ap.add_argument("--max-steps", type=int, default=0, help="train: stop early (smoke test)")
    ap.add_argument("--work", default="work")
    args = ap.parse_args()
    global WORK
    WORK = Path(args.work)
    WORK.mkdir(exist_ok=True)
    {"gen": cmd_gen, "styles": cmd_styles, "pairs": cmd_pairs, "train": cmd_train,
     "export": cmd_export, "check": cmd_check}[args.step](args)


if __name__ == "__main__":
    main()
