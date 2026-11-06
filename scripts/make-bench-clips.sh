#!/usr/bin/env bash
# Generates the synthetic benchmark set into bench/clips (gitignored): <id>.wav + <id>.txt (reference).
# Uses macOS `say`, so it only runs on a Mac. Real recordings with hand-written references can be
# dropped into the same folder (<id>.wav + <id>.txt) and `umless bench` will include them.
#
# Format: id|voice|words-per-minute|spoken text|clean text.
# [[slnc N]] inserts N ms of silence and is not part of the reference.
# <id>.txt is what was said (scores the ASR); <id>.clean.txt is what the user meant (scores ASR + cleanup).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/bench/clips"
mkdir -p "$OUT"

CLIPS=(
"dictation|Samantha|170|Um, so hey Priya, can we uh push the the Q3 roadmap review to Thursday, no wait, Friday? And also, uh, tell Gaurav that the Kubernetes migration is basically done, like ninety percent, and we just need to, um, test the staging cluster before we ship it.|Hey Priya, can we push the Q3 roadmap review to Friday? And also, tell Gaurav that the Kubernetes migration is basically done, like 90%, and we just need to test the staging cluster before we ship it."
"pauses|Samantha|170|My name is Jatin, and I am a software engineer. [[slnc 4000]] I would like to create products every day [[slnc 3000]] and earn from them. [[slnc 2500]]|My name is Jatin, and I am a software engineer. I would like to create products every day and earn from them."
"fast|Samantha|215|So this uh goes back and forth with the general contractor and makes uh everything easier. This uh involves three types of things, uh billing apps, um going back and forth with the contractor and uh uh interacting with the foreman on the ground, and then uh the compliance forms and the lien waivers and um all the signatures that uh need to happen before anyone gets paid, which is uh honestly the biggest pain for uh every electrical subcontractor we have talked to so far.|So this goes back and forth with the general contractor and makes everything easier. This involves three types of things: billing apps, going back and forth with the contractor and interacting with the foreman on the ground, and then the compliance forms and the lien waivers and all the signatures that need to happen before anyone gets paid, which is honestly the biggest pain for every electrical subcontractor we have talked to so far."
"founder-in|Aman|175|Hi, I'm Jatin, founder of SubFlo. We build billing software for electrical subcontractors in the US. Uh, our agent handles pay applications, lien waivers and compliance forms, and it talks to the foreman and the general contractor so that payments are not delayed.|Hi, I'm Jatin, founder of SubFlo. We build billing software for electrical subcontractors in the US. Our agent handles pay applications, lien waivers and compliance forms, and it talks to the foreman and the general contractor so that payments are not delayed."
"email-gb|Daniel|180|Hi Sarah, thanks for the update. Could we move the design review to Wednesday at three pm? No, actually, make that Thursday morning. Also, please send me the latest invoice and the contract before the end of the week. Cheers.|Hi Sarah, thanks for the update. Could we move the design review to Thursday morning? Also, please send me the latest invoice and the contract before the end of the week. Cheers."
"numbers-in|Aman|165|The invoice total is twelve thousand four hundred dollars, due on the fifteenth of March, and the retainage is ten percent of the contract value.|The invoice total is \$12,400, due on the 15th of March, and the retainage is 10% of the contract value."
"tech-gb|Daniel|185|Let's deploy the Kubernetes cluster with Terraform, then check the Postgres replica and the Redis cache on staging before we merge the pull request.|Let's deploy the Kubernetes cluster with Terraform, then check the Postgres replica and the Redis cache on staging before we merge the pull request."
"names-us|Samantha|175|Can you loop in Siobhan and Priya Raghunathan on the Anthropic thread, and ask Gaurav to share the Figma file with Nikhil before Thursday?|Can you loop in Siobhan and Priya Raghunathan on the Anthropic thread, and ask Gaurav to share the Figma file with Nikhil before Thursday?"
"names-in|Aman|170|Please tell Arjun Mehta that the Zerodha integration and the Razorpay webhook are ready, and that Jatin Bishla will demo SubFlo to the team tomorrow.|Please tell Arjun Mehta that the Zerodha integration and the Razorpay webhook are ready, and that Jatin Bishla will demo SubFlo to the team tomorrow."
"jargon-gb|Daniel|180|The foreman signed the conditional lien waiver, so the schedule of values and the retainage release can go to Turner Construction today.|The foreman signed the conditional lien waiver, so the schedule of values and the retainage release can go to Turner Construction today."
"short|Samantha|175|Sounds good, see you tomorrow.|Sounds good, see you tomorrow."
)

for row in "${CLIPS[@]}"; do
  IFS='|' read -r id voice rate text clean <<<"$row"
  # Not every Mac has every voice installed; fall back to one that ships with macOS.
  say -v '?' | grep -q "^$voice " || voice=Samantha
  say -v "$voice" -r "$rate" -o "$OUT/$id.aiff" "$text"
  afconvert -f WAVE -d LEI16@16000 "$OUT/$id.aiff" "$OUT/$id.wav"
  rm "$OUT/$id.aiff"
  # Reference = the spoken text without embedded say commands.
  sed -E 's/\[\[[^]]*\]\]//g; s/  +/ /g; s/ +$//' <<<"$text" >"$OUT/$id.txt"
  echo "$clean" >"$OUT/$id.clean.txt"
  echo "✓ $id ($voice)"
done
