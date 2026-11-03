# Dictionary (hotwords): all tiers, 2026-11-03

`muskflow bench --tiers lite,standard,max --fast --show --dictionary bench/dictionary.txt` on the 11 synthetic clips. Without the dictionary, final WER was 10.5% / 7.2% / 4.2% (Lite / Standard / Max).

## Tier comparison (fast replay)

| Tier | Models | Size | Load | **Final WER** (vs meant) | Gap | Rules-only WER | ASR WER (vs said) | Gap | Key-up → text p50 / max | Cleanup p50 | Guard fallbacks | CPU while speaking |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| **lite** | Parakeet-TDT 110M int8 (English) + Qwen3.5-0.8B-Q4_K_M | 669 MB | 1.5 s | **6.6%** | +5.1 pts | 9.3% | 9.2% | +5.8 pts | 732 / 1549 ms | 563 ms | 2/11 | 19% |
| **standard** | Parakeet-TDT 0.6B v3 int8 (25 European languages) + Qwen3.5-2B-Q4_K_M | 1951 MB | 3.2 s | **3.6%** | +2.1 pts | 6.9% | 6.3% | +2.9 pts | 1653 / 3201 ms | 1169 ms | 0/11 | 52% |
| **max** | Whisper large-v3-turbo (99 languages) + Qwen3-4B-Instruct-2507-Q4_K_M | 3533 MB | 7.6 s | **1.5%** | — | 4.5% | 3.5% | — | 5004 / 8153 ms | 979 ms | 0/11 | 86% |

### Per clip: final WER (ASR WER)

| Clip | lite | standard | max |
|---|---|---|---|
| dictation | 15.4% (6.8%) | 5.1% (4.5%) | 2.6% (4.5%) |
| email-gb | 3.1% (2.5%) | 0.0% (7.5%) | 0.0% (0.0%) |
| fast | 10.8% (12.2%) | 4.1% (5.4%) | 4.1% (1.4%) |
| founder-in | 0.0% (4.8%) | 0.0% (9.5%) | 0.0% (0.0%) |
| jargon-gb | 0.0% (9.1%) | 4.5% (4.5%) | 0.0% (4.5%) |
| names-in | 8.0% (16.0%) | 0.0% (16.0%) | 0.0% (12.0%) |
| names-us | 4.2% (25.0%) | 4.2% (12.5%) | 0.0% (16.7%) |
| numbers-in | 0.0% (0.0%) | 16.0% (0.0%) | 0.0% (0.0%) |
| pauses | 4.5% (4.5%) | 4.5% (4.5%) | 4.5% (4.5%) |
| short | 0.0% (0.0%) | 0.0% (0.0%) | 0.0% (0.0%) |
| tech-gb | 12.5% (16.7%) | 0.0% (0.0%) | 0.0% (0.0%) |

### Transcripts

**dictation** (bench/clips/dictation.wav)

- said: Um, so hey Priya, can we uh push the the Q3 roadmap review to Thursday, no wait, Friday? And also, uh, tell Gaurav that the Kubernetes migration is basically done, like ninety percent, and we just need to, um, test the staging cluster before we ship it.
- meant: Hey Priya, can we push the Q3 roadmap review to Friday? And also, tell Gaurav that the Kubernetes migration is basically done, like 90%, and we just need to test the staging cluster before we ship it.
- lite ASR: So hey pria, can we up push the the Q three roadmap review to Thursday? No. wait, Friday and also Ah, tell Garv that the Kubernetes migration is basically done like ninety percent and we just need to. um, test the staging cluster before we ship it.
- lite final (fallback: dropped words the speaker didn't take back; llm said "So, can we push the Q3 roadmap review to Thursday? No. Wait, Friday, and also tell Gaurav that the Kubernetes migration is basically done like ninety percent, and we just need to test the staging cluster before we ship it."): So hey pria, can we up push the Q three roadmap review to Thursday? No. wait, Friday and also tell Gaurav that the Kubernetes migration is basically done like ninety percent and we just need to. test the staging cluster before we ship it.
- standard ASR: Um, so hey Priya, can we up push the the Q3 roadmap review to Thursday, no wait, Friday, and also, uh, tell Garov that the Kubernetes migration is basically done, like 90%, and we just need to, um, test the staging cluster before we ship it.
- standard final (llm): So hey Priya, can we push the Q3 roadmap review to Friday? Also, tell Gaurav that the Kubernetes migration is basically done, like 90%, and we just need to test the staging cluster before we ship it.
- max ASR: So hey Priya, can we up-push the The Q3 Roadmap review to Thursday? No wait, Friday? And also, uh, tell Garv that the Kubernetes migration is basically done. like 90%, and we just need to, um, test the staging cluster before we ship it.
- max final (llm): So hey Priya, can we push the Q3 Roadmap review to Friday? And also, tell Gaurav that the Kubernetes migration is basically done—like 90%—and we just need to test the staging cluster before we ship it.

**email-gb** (bench/clips/email-gb.wav)

- said: Hi Sarah, thanks for the update. Could we move the design review to Wednesday at three pm? No, actually, make that Thursday morning. Also, please send me the latest invoice and the contract before the end of the week. Cheers.
- meant: Hi Sarah, thanks for the update. Could we move the design review to Thursday morning? Also, please send me the latest invoice and the contract before the end of the week. Cheers.
- lite ASR: Hi Sarah, thanks for the update. Could we move the design review to Wednesday at three PM? No, actually, make that Thursday morning. Also, please send me the latest invoice in the contract before the end of the week. Cheers.
- lite final (llm): Hi Sarah, thanks for the update. Could we move the design review to Thursday morning? Also, please send me the latest invoice in the contract before the end of the week. Cheers.
- standard ASR: Hi Sarah, Thanks the update. Could we move the move the design review to Wednesday at 3 pm? No, Actually, make that Thursday morning. Also, please send me the latest invoice and the contract before the end of the week. Cheers.
- standard final (llm): Hi Sarah, thanks for the update. Could we move the design review to Thursday morning? Also, please send me the latest invoice and the contract before the end of the week. Cheers.
- max ASR: Hi Sarah, thanks for the update. Could we move the design review to Wednesday at 3pm? No, actually, make that Thursday morning. Also, please send me the latest invoice and the contract before the end of the week. Cheers.
- max final (llm): Hi Sarah, thanks for the update. Could we move the design review to Thursday morning? Also, please send me the latest invoice and the contract before the end of the week. Cheers.

**fast** (bench/clips/fast.wav)

- said: So this uh goes back and forth with the general contractor and makes uh everything easier. This uh involves three types of things, uh billing apps, um going back and forth with the contractor and uh uh interacting with the foreman on the ground, and then uh the compliance forms and the lien waivers and um all the signatures that uh need to happen before anyone gets paid, which is uh honestly the biggest pain for uh every electrical subcontractor we have talked to so far.
- meant: So this goes back and forth with the general contractor and makes everything easier. This involves three types of things: billing apps, going back and forth with the contractor and interacting with the foreman on the ground, and then the compliance forms and the lien waivers and all the signatures that need to happen before anyone gets paid, which is honestly the biggest pain for every electrical subcontractor we have talked to so far.
- lite ASR: So this all goes back and forth with the general contractor and makes up everything easier. This involves three types of things all billing apps, I'm going back and forth with the contractor and not interacting with the foreman on the ground, and then other compliance forms and the lean waivers and un all the signatures that I need to happen before anyone gets paid, which is honestly the biggest pain for every electrical subcontractor we have talked to so far.
- lite final (fallback: dropped words the speaker didn't take back; llm said "So this all goes back and forth with the general contractor, making things easier. This involves three types of things: all billing apps, I'm going back and forth with the contractor, and not interacting with the foreman on the ground. Then other compliance forms and the lien waivers and unall the signatures that I need to happen before anyone gets paid, which is honestly the biggest pain for every electrical subcontractor we have talked to so far."): So this all goes back and forth with the general contractor and makes up everything easier. This involves three types of things all billing apps, I'm going back and forth with the contractor and not interacting with the foreman on the ground, and then other compliance forms and the lien waivers and un all the signatures that I need to happen before anyone gets paid, which is honestly the biggest pain for every electrical subcontractor we have talked to so far.
- standard ASR: So this uh goes back and forth with the general contractor and makes uh everything easier. This a involves three types of things. I'll billing apps, I'm going back and forth with the contractor and uh interacting with the foreman on the ground, and then uh the compliance forms and the lean waivers and um all the signatures that uh need to happen before anyone gets paid, which is honestly the biggest pain for every electrical subcontractor we have talked to so far.
- standard final (llm): So this goes back and forth with the general contractor and makes everything easier. This involves three types of things: I'll billing apps, I'm going back and forth with the contractor, and interacting with the foreman on the ground, and then the compliance forms and the lien waivers and all the signatures that need to happen before anyone gets paid. This is honestly the biggest pain for every electrical subcontractor we have talked to so far.
- max ASR: So this goes back and forth with the general contractor and makes everything easier. This involves three types of things. Billing apps. I'm going back and forth with the contractor and interacting with the foreman on the ground. And then the compliance forms and the lien waivers and all the signatures that need to happen before anyone gets paid, which is honestly the biggest pain for every electrical subcontractor we have talked to so far.
- max final (llm): So this goes back and forth with the general contractor and makes everything easier. This involves three types of things: billing apps, I'm going back and forth with the contractor and interacting with the foreman on the ground, and then the compliance forms and the lien waivers and all the signatures that need to happen before anyone gets paid, which is honestly the biggest pain for every electrical subcontractor we've talked to so far.

**founder-in** (bench/clips/founder-in.wav)

- said: Hi, I'm Jatin, founder of SubFlo. We build billing software for electrical subcontractors in the US. Uh, our agent handles pay applications, lien waivers and compliance forms, and it talks to the foreman and the general contractor so that payments are not delayed.
- meant: Hi, I'm Jatin, founder of SubFlo. We build billing software for electrical subcontractors in the US. Our agent handles pay applications, lien waivers and compliance forms, and it talks to the foreman and the general contractor so that payments are not delayed.
- lite ASR: Hi, I'm Jatin, founder of Subflow. We build billing software for electrical subcontractors in the US are our agent handles pay applications, lien waivers and compliance forms and it talks to the foreman and the general contractor so that payments are not delayed.
- lite final (llm): Hi, I'm Jatin, founder of SubFlo. We build billing software for electrical subcontractors in the US. Our agent handles pay applications, lien waivers, and compliance forms, and it talks to the foreman and the general contractor so that payments are not delayed.
- standard ASR: Hi, I'm Jatin, founder of Subflow. We build billing software for electrical subcontractors in the US are our agent handles pay applications, lean waivers and compliance forms, and it talks to the firmman and the general contractor so that payments are not delayed.
- standard final (llm): Hi, I'm Jatin, founder of SubFlo. We build billing software for electrical subcontractors in the US. Our agent handles pay applications, lien waivers, and compliance forms, and it talks to the foreman and the general contractor so that payments are not delayed.
- max ASR: Hi, I'm Jatin, founder of Subflo. We build billing software for electrical subcontractors in the US. Our agent handles pay applications. lien waivers and compliance forms. And it talks to the foreman and the general contractor so that payments are not delayed.
- max final (llm): Hi, I'm Jatin, founder of SubFlo. We build billing software for electrical subcontractors in the US. Our agent handles pay applications, lien waivers, and compliance forms, and it talks to the foreman and the general contractor so that payments are not delayed.

**jargon-gb** (bench/clips/jargon-gb.wav)

- said: The foreman signed the conditional lien waiver, so the schedule of values and the retainage release can go to Turner Construction today.
- meant: The foreman signed the conditional lien waiver, so the schedule of values and the retainage release can go to Turner Construction today.
- lite ASR: The foreman signed the conditional lien waiver, so the schedule of values and the retainage release can go to turn a construction today.
- lite final (llm): The foreman signed the conditional lien waiver, so the schedule of values and the retainage release can go to Turner Construction today.
- standard ASR: The foreman signed the conditional waiver, so the schedule of values and the retainage release can go to turner construction today.
- standard final (llm): The foreman signed the conditional waiver, so the schedule of values and the retainage release can go to Turner Construction today.
- max ASR: The foreman signed the conditional LEAN waiver, so the schedule of values and the retainage release can go to Turner Construction today.
- max final (llm): The foreman signed the conditional lien waiver, so the schedule of values and the retainage release can go to Turner Construction today.

**names-in** (bench/clips/names-in.wav)

- said: Please tell Arjun Mehta that the Zerodha integration and the Razorpay webhook are ready, and that Jatin Bishla will demo SubFlo to the team tomorrow.
- meant: Please tell Arjun Mehta that the Zerodha integration and the Razorpay webhook are ready, and that Jatin Bishla will demo SubFlo to the team tomorrow.
- lite ASR: Please tell Arjun Meta that the Zirodha integration and the Razape webhook are ready and that Jatin Bishla will demo subfloat to the team tomorrow.
- lite final (llm): Please tell Arjun Mehta that the Zerodha integration and the Razape webhook are ready, and that Jatin Bishla will demo subfloat to the team tomorrow.
- standard ASR: Please tell Arjun Mahta that the Zirudha integration and the Razapay webhook are ready, and that Jatin Bishla will demo subflow to the team tomorrow.
- standard final (llm): Please tell Arjun Mehta that the Zerodha integration and the Razorpay webhook are ready, and that Jatin Bishla will demo SubFlo to the team tomorrow.
- max ASR: Please tell Arjun Mehta that the Zerodha integration and the Razerpay webhook are ready. And that Jatin Bishla will demo sub flow to the team tomorrow.
- max final (llm): Please tell Arjun Mehta that the Zerodha integration and the Razorpay webhook are ready. And that Jatin Bishla will demo SubFlo to the team tomorrow.

**names-us** (bench/clips/names-us.wav)

- said: Can you loop in Siobhan and Priya Raghunathan on the Anthropic thread, and ask Gaurav to share the Figma file with Nikhil before Thursday?
- meant: Can you loop in Siobhan and Priya Raghunathan on the Anthropic thread, and ask Gaurav to share the Figma file with Nikhil before Thursday?
- lite ASR: Can you loop in Chivan and Pria Ragunathan on the anthropic thread and ask Garv to share the figma file with Nick Hill before Thursday.
- lite final (llm): Can you loop in Chivan and Priya Raghunathan on the Anthropic thread and ask Gaurav to share the Figma file with Nikhil before Thursday?
- standard ASR: Can you loop in Shivan and Priya Ragunathan on the anthropic thread, and ask Garov to share the Figma file with Nikhil before Thursday?
- standard final (llm): Can you loop in Shivan and Priya Raghunathan on the Anthropic thread, and ask Gaurav to share the Figma file with Nikhil before Thursday?
- max ASR: Can you loop in Siobhan and Priya Ragunathan on the Anthropic thread, and ask Garv to share the Figma file with Nick Hill before Thursday?
- max final (llm): Can you loop in Siobhan and Priya Raghunathan on the Anthropic thread, and ask Gaurav to share the Figma file with Nikhil before Thursday?

**numbers-in** (bench/clips/numbers-in.wav)

- said: The invoice total is twelve thousand four hundred dollars, due on the fifteenth of March, and the retainage is ten percent of the contract value.
- meant: The invoice total is $12,400, due on the 15th of March, and the retainage is 10% of the contract value.
- lite ASR: The invoice total IS twelve thousand four hundred dollars due on the fifteenth of march and the retainage is ten percent of the contract value.
- lite final (llm): The invoice total is $12,400, due on the fifteenth of March, and the retainage is 10% of the contract value.
- standard ASR: The invoice total is $12,400 due on the 15th of March, and the retainage is 10% of the contract value.
- standard final (llm): The invoice total is $12,400 due on March 15, and the retainage is 10% of the contract value.
- max ASR: The invoice total is $12,400 due on the 15th of March And the retainage is 10% of the contract value.
- max final (llm): The invoice total is $12,400, due on the 15th of March, and the retainage is 10% of the contract value.

**pauses** (bench/clips/pauses.wav)

- said: My name is Jatin, and I am a software engineer. I would like to create products every day and earn from them.
- meant: My name is Jatin, and I am a software engineer. I would like to create products every day and earn from them.
- lite ASR: My name is Joden, and I am a software engineer. I would like to create products every day. And earn from them.
- lite final (llm): My name is Joden, and I am a software engineer. I would like to create products every day and earn from them.
- standard ASR: My name is Jaden, and I am a software engineer. I would like to create products every day. And earn from them.
- standard final (llm): My name is Jaden, and I am a software engineer. I would like to create products every day and earn from them.
- max ASR: My name is Jodan, and I am a software engineer. I would like to create products every day and earn from them.
- max final (llm): My name is Jodan, and I am a software engineer. I would like to create products every day and earn from them.

**short** (bench/clips/short.wav)

- said: Sounds good, see you tomorrow.
- meant: Sounds good, see you tomorrow.
- lite ASR: Sounds good, see you tomorrow.
- lite final (rules): Sounds good, see you tomorrow.
- standard ASR: Sounds good. See you tomorrow.
- standard final (rules): Sounds good. See you tomorrow.
- max ASR: Sounds good. See you tomorrow.
- max final (rules): Sounds good. See you tomorrow.

**tech-gb** (bench/clips/tech-gb.wav)

- said: Let's deploy the Kubernetes cluster with Terraform, then check the Postgres replica and the Redis cache on staging before we merge the pull request.
- meant: Let's deploy the Kubernetes cluster with Terraform, then check the Postgres replica and the Redis cache on staging before we merge the pull request.
- lite ASR: Let's deploy the Kubernetes cluster with terraform, then check the postger's replica and the Rediscon staging before we merge the pull request.
- lite final (llm): Let's deploy the Kubernetes cluster with Terraform, then check the Postgres replica and the Rediscon staging before we merge the pull request.
- standard ASR: Let's deploy the Kubernetes cluster with Terraform, then check the Postgres replica and the Redis cache on staging before we merge the pull request.
- standard final (llm): Let's deploy the Kubernetes cluster with Terraform, then check the Postgres replica and the Redis cache on staging before we merge the pull request.
- max ASR: Let's deploy the Kubernetes cluster with Terraform. Then check the Postgres replica and the Redis cache on staging before we merge the pull request.
- max final (llm): Let's deploy the Kubernetes cluster with Terraform. Then check the Postgres replica and the Redis cache on staging before we merge the pull request.


WER ignores case, punctuation, number formatting and fillers. Final WER also counts uncorrected self-corrections ("Thursday, no wait, Friday" should become "Friday") and anything the LLM added.
