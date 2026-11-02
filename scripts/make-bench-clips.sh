#!/usr/bin/env bash
# Generates the synthetic benchmark set into bench/clips (gitignored): <id>.wav + <id>.txt (reference).
# Uses macOS `say`, so it only runs on a Mac. Real recordings with hand-written references can be
# dropped into the same folder (<id>.wav + <id>.txt) and `muskflow bench` will include them.
#
# Format: id|voice|words-per-minute|text. [[slnc N]] inserts N ms of silence and is not part of the reference.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/bench/clips"
mkdir -p "$OUT"

CLIPS=(
"dictation|Samantha|170|Um, so hey Priya, can we uh push the the Q3 roadmap review to Thursday, no wait, Friday? And also, uh, tell Gaurav that the Kubernetes migration is basically done, like ninety percent, and we just need to, um, test the staging cluster before we ship it."
"pauses|Samantha|170|My name is Jatin, and I am a software engineer. [[slnc 4000]] I would like to create products every day [[slnc 3000]] and earn from them. [[slnc 2500]]"
"fast|Samantha|215|So this uh goes back and forth with the general contractor and makes uh everything easier. This uh involves three types of things, uh billing apps, um going back and forth with the contractor and uh uh interacting with the foreman on the ground, and then uh the compliance forms and the lien waivers and um all the signatures that uh need to happen before anyone gets paid, which is uh honestly the biggest pain for uh every electrical subcontractor we have talked to so far."
"founder-in|Aman|175|Hi, I'm Jatin, founder of SubFlo. We build billing software for electrical subcontractors in the US. Uh, our agent handles pay applications, lien waivers and compliance forms, and it talks to the foreman and the general contractor so that payments are not delayed."
"email-gb|Daniel|180|Hi Sarah, thanks for the update. Could we move the design review to Wednesday at three pm? No, actually, make that Thursday morning. Also, please send me the latest invoice and the contract before the end of the week. Cheers."
"numbers-in|Aman|165|The invoice total is twelve thousand four hundred dollars, due on the fifteenth of March, and the retainage is ten percent of the contract value."
"tech-gb|Daniel|185|Let's deploy the Kubernetes cluster with Terraform, then check the Postgres replica and the Redis cache on staging before we merge the pull request."
"short|Samantha|175|Sounds good, see you tomorrow."
)

for row in "${CLIPS[@]}"; do
  IFS='|' read -r id voice rate text <<<"$row"
  say -v "$voice" -r "$rate" -o "$OUT/$id.aiff" "$text"
  afconvert -f WAVE -d LEI16@16000 "$OUT/$id.aiff" "$OUT/$id.wav"
  rm "$OUT/$id.aiff"
  # Reference = the spoken text without embedded say commands.
  sed -E 's/\[\[[^]]*\]\]//g; s/  +/ /g; s/ +$//' <<<"$text" >"$OUT/$id.txt"
  echo "✓ $id ($voice)"
done
