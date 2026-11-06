//! Streaming on top of an offline transcriber (LocalAgreement-2).
//!
//! Every `step` of new audio we re-decode the *uncommitted window*. Words that
//! two consecutive decodes agree on (ignoring case and punctuation) are
//! committed and the window slides past them. On key-up, [`Committer::finish`]
//! only has to decode the short uncommitted tail, which is what keeps
//! key-up → text latency low regardless of how long the user spoke.

use std::time::Instant;

use anyhow::Result;
use umless_audio::{SpeechSegmenter, SpeechTrimmer};

use crate::{Hypothesis, Session, SessionStats, Transcriber, Word, join_words};

const SAMPLE_RATE: f32 = 16_000.0;
/// Audio kept in the window when a decode finds no words.
const SILENCE_KEEP_SECS: f32 = 0.5;
/// Audio kept before a word when snapping the context start to it.
const WORD_LEAD_SECS: f32 = 0.05;
/// Empty decodes tolerated while VAD still hears speech.
const MAX_EMPTY_OVER_SPEECH: u32 = 4;
/// Speech this long with no word in it means the streaming decodes lost it.
const MISSED_SPEECH_SECS: f32 = 1.0;
/// Audio decoded before a missed stretch, so the model doesn't start mid-word.
const RESCUE_LEAD_SECS: f32 = 1.0;
const VAD_LATE_SECS: f32 = 0.5;

#[derive(Debug, Clone)]
pub struct CommitterConfig {
    /// Re-decode after this much new audio.
    pub step_secs: f32,
    /// Don't decode windows shorter than this; the model needs some context.
    pub min_window_secs: f32,
    /// If nothing has been agreed for this long, force-commit everything except the last `keep_secs`.
    pub max_window_secs: f32,
    pub keep_secs: f32,
    /// Already-committed audio re-fed before the window so the model keeps sentence
    /// context (casing, punctuation, no re-heard word fragments). Words that land in
    /// it are discarded.
    pub context_secs: f32,
}

impl Default for CommitterConfig {
    fn default() -> Self {
        Self {
            step_secs: 1.0,
            min_window_secs: 1.0,
            max_window_secs: 6.0,
            keep_secs: 2.0,
            context_secs: 1.5,
        }
    }
}

pub struct Committer<'a> {
    asr: &'a dyn Transcriber,
    /// Decides whether an empty decode is silence or a model dropout.
    vad: Option<&'a SpeechTrimmer>,
    /// Marks speech as it arrives, so key-up can find stretches the decodes lost.
    segmenter: Option<SpeechSegmenter>,
    /// Speech found so far, as `[start, end)` sample ranges.
    speech: Vec<(usize, usize)>,
    hotwords: Vec<String>,
    cfg: CommitterConfig,
    audio: Vec<f32>,
    /// Start of the audio still being decoded. Silence moves it forward too.
    window_start: usize,
    /// End of the audio whose words are committed. Only words before this are discarded
    /// from re-decoded context; audio skipped as silence may still hold a word onset.
    committed_end: usize,
    last_decode_at: usize,
    /// Consecutive decodes that returned no words.
    empty_streak: u32,
    committed: Vec<Word>,
    pending: Vec<Word>,
    pub stats: SessionStats,
}

impl<'a> Committer<'a> {
    pub fn new(asr: &'a dyn Transcriber, hotwords: Vec<String>, cfg: CommitterConfig) -> Self {
        Self {
            asr,
            vad: None,
            segmenter: None,
            speech: Vec::new(),
            hotwords,
            cfg,
            audio: Vec::with_capacity(16_000 * 60),
            window_start: 0,
            committed_end: 0,
            last_decode_at: 0,
            empty_streak: 0,
            committed: Vec::new(),
            pending: Vec::new(),
            stats: SessionStats::default(),
        }
    }

    /// Uses voice activity detection to tell silence from a decode that dropped speech.
    pub fn with_vad(mut self, vad: &'a SpeechTrimmer) -> Self {
        self.vad = Some(vad);
        self
    }

    /// Watches the whole take for speech, so stretches the decodes lost can be decoded
    /// again at key-up.
    pub fn with_segmenter(mut self, segmenter: SpeechSegmenter) -> Self {
        self.segmenter = Some(segmenter);
        self
    }

    fn add_audio(&mut self, pcm: &[f32]) {
        self.audio.extend_from_slice(pcm);
        if let Some(seg) = self.segmenter.as_mut() {
            let found = seg.push(pcm);
            self.speech
                .extend(found.iter().map(|s| (s.start, s.start + s.samples.len())));
        }
    }

    /// Whether the uncommitted audio is silence, after `empty_streak` empty decodes.
    fn is_silence(&self) -> bool {
        match self.vad {
            // Trust VAD, but give up after a few empty decodes over "speech" (noise, coughs)
            // so the window can't grow without bound.
            Some(vad) => {
                self.empty_streak >= MAX_EMPTY_OVER_SPEECH
                    || vad
                        .speech_bounds(&self.audio[self.window_start..])
                        .is_none()
            }
            None => self.empty_streak >= 2,
        }
    }

    fn step(&mut self) -> Result<Vec<Word>> {
        self.last_decode_at = self.audio.len();
        let hyp = self.decode_window()?;

        if hyp.is_empty() {
            // An empty decode can be a dropout mid-speech, so keep what we were waiting to
            // confirm. Only real silence stops us re-decoding it; leave a little audio in
            // case a word is just starting.
            self.empty_streak += 1;
            if self.is_silence() {
                let keep = (SILENCE_KEEP_SECS * SAMPLE_RATE) as usize;
                self.window_start = self.window_start.max(self.audio.len().saturating_sub(keep));
                self.pending.clear();
            }
            return Ok(Vec::new());
        }
        self.empty_streak = 0;

        let mut agreed = agree(&self.pending, &hyp);
        log::debug!(
            "t={:.1}s commit={:?} | prev: {} | now: {}",
            self.audio_secs(),
            join_words(&agreed.commit),
            join_words(&self.pending),
            join_words(&hyp)
        );
        let window_secs = (self.audio.len() - self.window_start) as f32 / SAMPLE_RATE;
        if agreed.commit.is_empty() && window_secs > self.cfg.max_window_secs {
            // Long unstable stretch (noise, mumbling): don't let the tail grow without bound.
            let horizon = self.audio_secs() - self.cfg.keep_secs;
            let n = hyp.iter().take_while(|w| w.end <= horizon).count();
            agreed = Agreement {
                commit: hyp[..n].to_vec(),
                now_end: n,
            };
        }
        if agreed.commit.is_empty() {
            self.pending = hyp;
            return Ok(Vec::new());
        }

        let Agreement {
            commit: newly,
            now_end,
        } = agreed;
        let last_end = newly.last().map(|w| w.end).unwrap_or(0.0);
        // Cut halfway into the gap before the next word so neither word loses audio.
        let cut = match hyp.get(now_end) {
            Some(next) => (last_end + next.start) / 2.0,
            None => last_end,
        };
        let cut =
            ((cut.max(0.0) * SAMPLE_RATE) as usize).clamp(self.committed_end, self.audio.len());
        self.committed_end = cut;
        self.window_start = self.window_start.max(cut);
        self.pending = hyp[now_end..].to_vec();
        self.committed.extend(newly.iter().cloned());
        Ok(newly)
    }

    /// Where to start feeding audio: about `context_secs` before the window, snapped
    /// back to the start of a committed word. Starting a decode mid-word can make
    /// Parakeet drop the next several seconds of speech entirely.
    fn context_start(&self) -> usize {
        let target = self
            .window_start
            .saturating_sub((self.cfg.context_secs * SAMPLE_RATE) as usize);
        let target_secs = target as f32 / SAMPLE_RATE;
        let word_start = self
            .committed
            .iter()
            .rev()
            .take_while(|w| w.end > target_secs)
            .last()
            .map(|w| ((w.start - WORD_LEAD_SECS).max(0.0) * SAMPLE_RATE) as usize);
        word_start.map_or(target, |s| s.min(target))
    }

    /// Decodes the uncommitted window (plus left context) and returns only the
    /// new words, with absolute timestamps.
    fn decode_window(&mut self) -> Result<Vec<Word>> {
        let from = self.context_start();
        let offset = from as f32 / SAMPLE_RATE;
        let boundary = self.committed_end as f32 / SAMPLE_RATE;

        let t = Instant::now();
        let hyp = self.asr.transcribe(&self.audio[from..], &self.hotwords)?;
        self.stats.decodes += 1;
        self.stats.decode_time += t.elapsed();

        log::trace!(
            "raw from {:.2}s, boundary {:.2}s: {}",
            offset,
            boundary,
            hyp.words
                .iter()
                .map(|w| format!("{}@{:.2}-{:.2}", w.text, w.start + offset, w.end + offset))
                .collect::<Vec<_>>()
                .join(" ")
        );
        let mut words: Vec<Word> = hyp
            .words
            .into_iter()
            .map(|w| Word {
                start: w.start + offset,
                end: w.end + offset,
                ..w
            })
            .filter(|w| (w.start + w.end) / 2.0 >= boundary)
            .collect();
        // A word straddling the cut can be heard on both sides; keep the committed copy.
        if let (Some(last), Some(first)) = (self.committed.last(), words.first())
            && norm(&last.text) == norm(&first.text)
            && first.start < boundary + 0.3
        {
            words.remove(0);
        }
        Ok(words)
    }
}

impl Session for Committer<'_> {
    /// Adds audio. Returns the words newly committed by this call (often none).
    fn append(&mut self, pcm: &[f32]) {
        self.add_audio(pcm);
    }

    fn push(&mut self, pcm: &[f32]) -> Result<Vec<Word>> {
        self.add_audio(pcm);
        let since_last = self.audio.len() - self.last_decode_at;
        let window = self.audio.len() - self.window_start;
        if (since_last as f32) < self.cfg.step_secs * SAMPLE_RATE
            || (window as f32) < self.cfg.min_window_secs * SAMPLE_RATE
        {
            return Ok(Vec::new());
        }
        self.step()
    }

    /// Committed text plus the current unconfirmed guess, for live display.
    fn partial_text(&self) -> (String, String) {
        (join_words(&self.committed), join_words(&self.pending))
    }

    fn audio_secs(&self) -> f32 {
        self.audio.len() as f32 / SAMPLE_RATE
    }

    /// Decodes whatever is still uncommitted and returns the full transcript.
    fn finish(mut self: Box<Self>) -> Result<(Hypothesis, SessionStats)> {
        let tail = &self.audio[self.window_start..];
        self.stats.tail_secs = tail.len() as f32 / SAMPLE_RATE;
        let t = Instant::now();
        let hyp = self.decode_window()?;
        let pending = std::mem::take(&mut self.pending);
        self.committed.extend(restore_dropped_lead(&pending, hyp));
        if let Some(mut seg) = self.segmenter.take() {
            let found = seg.flush();
            let mut speech = std::mem::take(&mut self.speech);
            speech.extend(found.iter().map(|s| (s.start, s.start + s.samples.len())));
            self.rescue_missed_speech(&speech)?;
        }
        self.stats.tail_time = t.elapsed();
        let stats = self.stats.clone();
        Ok((
            Hypothesis {
                words: std::mem::take(&mut self.committed),
            },
            stats,
        ))
    }
}

impl Committer<'_> {
    /// A decode that starts badly (mid-word, a Bluetooth mic waking up) can lose
    /// seconds of speech for good once the window slides past it. At key-up, decode
    /// every stretch VAD calls speech but no word covers, and put its words back.
    fn rescue_missed_speech(&mut self, speech: &[(usize, usize)]) -> Result<()> {
        let speech: Vec<(f32, f32)> = speech
            .iter()
            .map(|&(s, e)| (s as f32 / SAMPLE_RATE, e as f32 / SAMPLE_RATE))
            .collect();
        for (lo, hi) in missed(&self.committed, &speech, MISSED_SPEECH_SECS) {
            let from = ((lo - RESCUE_LEAD_SECS).max(0.0) * SAMPLE_RATE) as usize;
            let to = ((hi * SAMPLE_RATE) as usize).min(self.audio.len());
            let offset = from as f32 / SAMPLE_RATE;
            let hyp = self.asr.transcribe(&self.audio[from..to], &self.hotwords)?;
            self.stats.decodes += 1;
            let found: Vec<Word> = hyp
                .words
                .into_iter()
                .map(|w| Word {
                    start: w.start + offset,
                    end: w.end + offset,
                    ..w
                })
                .filter(|w| (lo..hi).contains(&((w.start + w.end) / 2.0)))
                .collect();
            let at = self.committed.partition_point(|w| w.start < lo);
            let found = new_words(
                found,
                at.checked_sub(1).map(|i| &self.committed[i]),
                self.committed.get(at),
            );
            if found.is_empty() {
                continue;
            }
            log::info!(
                "rescued {:.1}–{:.1}s: {}",
                lo,
                hi,
                join_words(&found)
            );
            self.stats.rescued += 1;
            self.committed.splice(at..at, found);
        }
        Ok(())
    }
}

/// What a re-decode of a missed stretch adds: not the committed words on either side
/// heard again with other timings, and nothing if all it heard was hesitation.
fn new_words(mut found: Vec<Word>, before: Option<&Word>, after: Option<&Word>) -> Vec<Word> {
    let same = |a: &Word, b: Option<&Word>| b.is_some_and(|b| norm(&a.text) == norm(&b.text));
    if found.first().is_some_and(|w| same(w, before)) {
        found.remove(0);
    }
    if found.last().is_some_and(|w| same(w, after)) {
        found.pop();
    }
    let heard_words = found
        .iter()
        .any(|w| !UNSTABLE.contains(&norm(&w.text).as_str()));
    if heard_words { found } else { Vec::new() }
}

/// Stretches inside `speech` (seconds) at least `min_secs` long with no word in them,
/// bounded by the words on either side so a re-decode can't duplicate them. A stretch
/// may start up to [`VAD_LATE_SECS`] before VAD heard speech, in case it heard it late.
fn missed(words: &[Word], speech: &[(f32, f32)], min_secs: f32) -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    for &(start, end) in speech {
        let inside: Vec<&Word> = words
            .iter()
            .filter(|w| w.end > start && w.start < end)
            .collect();
        let mut lo = words
            .iter()
            .filter(|w| w.end <= start)
            .map(|w| w.end)
            .fold((start - VAD_LATE_SECS).max(0.0), f32::max);
        for w in inside.iter().map(|w| (w.start, w.end)).chain([(end, end)]) {
            let hi = w.0.min(end).max(lo);
            if hi - lo.max(start) >= min_secs {
                out.push((lo, hi));
            }
            lo = lo.max(w.1);
        }
    }
    out
}

fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Hesitations, plus the words the model confuses them with ("uh" ↔ "a", "um" ↔ "I'm").
/// They flip between decodes, so agreement ignores them; the cleanup LLM deals with them.
const UNSTABLE: &[&str] = &[
    "uh", "um", "ah", "er", "erm", "hm", "hmm", "mm", "mhm", "a", "i", "im",
];
/// How many leading words one decode may have that the other lacks (dropout at the window edge).
const MAX_LEAD_SKIP: usize = 3;

struct Agreement {
    /// Words to commit, taken from whichever decode had the fuller version.
    commit: Vec<Word>,
    /// Index in the newest decode just past the agreed span.
    now_end: usize,
}

/// The final decode has nothing after it to agree with. If it starts partway into
/// what the previous decode heard (window-edge dropout), put the missing words back.
fn restore_dropped_lead(prev: &[Word], now: Vec<Word>) -> Vec<Word> {
    let a = agree(prev, &now);
    match a.commit.first() {
        // `agree` took the leading words from `prev` only when `now` was missing them.
        Some(first) if !now.first().is_some_and(|n| n == first) && a.commit.len() > a.now_end => {
            let missing = a.commit.len() - a.now_end;
            prev[..missing].iter().cloned().chain(now).collect()
        }
        _ => now,
    }
}

/// LocalAgreement between the previous and the newest decode of the window.
///
/// Compares content words only (see [`UNSTABLE`]). Either decode may start with up to
/// [`MAX_LEAD_SKIP`] words the other dropped, as long as at least two words then agree;
/// the dropped words are kept, since dropout loses real words but rarely invents them.
fn agree(prev: &[Word], now: &[Word]) -> Agreement {
    let none = Agreement {
        commit: Vec::new(),
        now_end: 0,
    };
    let keys = |ws: &[Word]| -> Vec<(usize, String)> {
        ws.iter()
            .enumerate()
            .filter_map(|(i, w)| {
                let k = norm(&w.text);
                (!k.is_empty() && !UNSTABLE.contains(&k.as_str())).then_some((i, k))
            })
            .collect()
    };
    let (kp, kn) = (keys(prev), keys(now));

    // (skip in prev, skip in now, run length)
    let mut best: Option<(usize, usize, usize)> = None;
    for sp in 0..=MAX_LEAD_SKIP.min(kp.len()) {
        for sn in 0..=MAX_LEAD_SKIP.min(kn.len()) {
            if sp > 0 && sn > 0 {
                continue;
            }
            let run = kp[sp..]
                .iter()
                .zip(&kn[sn..])
                .take_while(|(a, b)| a.1 == b.1)
                .count();
            let needed = if sp + sn == 0 { 1 } else { 2 };
            if run >= needed && best.is_none_or(|b| run > b.2) {
                best = Some((sp, sn, run));
            }
        }
    }
    let Some((sp, sn, run)) = best else {
        return none;
    };

    let now_end = kn[sn + run - 1].0 + 1;
    let commit = if sp > 0 {
        prev[..=kp[sp + run - 1].0].to_vec()
    } else {
        now[..now_end].to_vec()
    };
    Agreement { commit, now_end }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Plays back a scripted sequence of hypotheses, one per decode call.
    struct Scripted(Mutex<Vec<Vec<(&'static str, f32, f32)>>>);

    impl Transcriber for Scripted {
        fn name(&self) -> &str {
            "scripted"
        }
        fn transcribe(&self, _pcm: &[f32], _h: &[String]) -> Result<Hypothesis> {
            let next = self.0.lock().unwrap().remove(0);
            Ok(Hypothesis {
                words: next
                    .into_iter()
                    .map(|(t, s, e)| Word {
                        text: t.into(),
                        start: s,
                        end: e,
                    })
                    .collect(),
            })
        }
    }

    #[test]
    fn commits_words_two_decodes_agree_on() {
        let asr = Scripted(Mutex::new(vec![
            vec![("Hello", 0.1, 0.4), ("word", 0.5, 0.9)],
            vec![("hello,", 0.1, 0.4), ("world", 0.5, 0.9), ("how", 1.0, 1.2)],
            // window now starts mid-gap after "hello" (0.45 s); timestamps are window-relative
            vec![
                ("world", 0.05, 0.45),
                ("how", 0.55, 0.75),
                ("are", 0.8, 1.0),
            ],
            vec![("are", 0.0, 0.2), ("you?", 0.3, 0.6)],
        ]));
        let cfg = CommitterConfig {
            step_secs: 0.5,
            context_secs: 0.0,
            ..Default::default()
        };
        let mut c = Box::new(Committer::new(&asr, vec![], cfg));
        let chunk = vec![0.0; 8_000]; // 0.5 s

        assert!(c.push(&chunk).unwrap().is_empty()); // window < 1 s: no decode
        assert!(c.push(&chunk).unwrap().is_empty()); // first decode, nothing to agree with
        let first = c.push(&chunk).unwrap();
        assert_eq!(
            first.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(),
            ["hello,"]
        );
        let second = c.push(&chunk).unwrap();
        assert_eq!(
            second.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(),
            ["world", "how"]
        );

        let (hyp, stats) = c.finish().unwrap();
        assert_eq!(hyp.text(), "hello, world how are you?");
        assert_eq!(stats.decodes, 4);
    }

    #[test]
    fn silence_slides_the_window_instead_of_growing_it() {
        let asr = Scripted(Mutex::new(vec![vec![], vec![]]));
        let cfg = CommitterConfig {
            context_secs: 0.0,
            ..Default::default()
        };
        let mut c = Committer::new(&asr, vec![], cfg);
        let second = vec![0.0; 16_000];
        c.push(&second).unwrap();
        c.push(&second).unwrap();
        // Two silent decodes: only the last 0.5 s stays in the window.
        assert_eq!(c.audio.len() - c.window_start, 8_000);
    }

    #[test]
    fn word_onset_in_skipped_silence_is_not_lost() {
        // Decode 1 (1 s of audio) hears only the start of a word: no words, window slides to 0.5 s.
        // Decode 2 (2 s, re-fed from 0 s as context) hears "My" at 0.2 s, before the slide
        // point. It was never committed, so it must be kept.
        let asr = Scripted(Mutex::new(vec![
            vec![],
            vec![("My", 0.2, 0.45), ("name", 0.5, 0.9)],
            vec![("My", 0.2, 0.45), ("name", 0.5, 0.9), ("is", 1.5, 1.7)],
        ]));
        let mut c = Box::new(Committer::new(&asr, vec![], CommitterConfig::default()));
        let second = vec![0.0; 16_000];
        c.push(&second).unwrap();
        c.push(&second).unwrap();
        let committed = c.push(&second).unwrap();
        assert_eq!(
            committed
                .iter()
                .map(|w| w.text.as_str())
                .collect::<Vec<_>>(),
            ["My", "name"]
        );
    }

    fn words(text: &str) -> Vec<Word> {
        text.split(' ')
            .enumerate()
            .map(|(i, t)| Word {
                text: t.into(),
                start: i as f32,
                end: i as f32 + 0.5,
            })
            .collect()
    }

    fn texts(a: &Agreement) -> String {
        join_words(&a.commit)
    }

    #[test]
    fn agreement_ignores_filler_flips() {
        let a = agree(
            &words("the contractor and uh interacting"),
            &words("the contractor and a interacting with"),
        );
        assert_eq!(texts(&a), "the contractor and a interacting");
        assert_eq!(a.now_end, 5);
    }

    #[test]
    fn agreement_keeps_words_the_newer_decode_dropped() {
        let prev = words("anyone gets paid, which is honestly the biggest pain for I ever");
        let now = words("Which is honestly the biggest pain for every electrical");
        let a = agree(&prev, &now);
        assert_eq!(
            texts(&a),
            "anyone gets paid, which is honestly the biggest pain for"
        );
        assert_eq!(now[a.now_end].text, "every");
    }

    #[test]
    fn agreement_keeps_words_the_older_decode_dropped() {
        let a = agree(
            &words("Which is honestly the"),
            &words("anyone gets paid, which is honestly the biggest"),
        );
        assert_eq!(texts(&a), "anyone gets paid, which is honestly the");
    }

    #[test]
    fn a_single_word_after_a_skip_is_not_enough() {
        assert!(
            agree(
                &words("which honestly"),
                &words("anyone gets paid, which is")
            )
            .commit
            .is_empty()
        );
    }

    #[test]
    fn final_decode_gets_dropped_lead_words_back() {
        let prev = words("every electrical subconscious.");
        let now = words("electrical subcontractor we have talked to");
        // Only one content word agrees after the skip, so nothing is restored.
        assert_eq!(
            join_words(&restore_dropped_lead(&prev, now.clone())),
            join_words(&now)
        );

        let prev = words("anyone gets paid, which is honestly");
        let now = words("which is honestly the biggest");
        assert_eq!(
            join_words(&restore_dropped_lead(&prev, now)),
            "anyone gets paid, which is honestly the biggest"
        );
    }

    #[test]
    fn one_empty_decode_is_not_silence() {
        let asr = Scripted(Mutex::new(vec![
            vec![("anyone", 0.1, 0.4), ("gets", 0.5, 0.7)],
            vec![],
            vec![("anyone", 0.1, 0.4), ("gets", 0.5, 0.7), ("paid", 0.8, 1.2)],
        ]));
        let cfg = CommitterConfig {
            context_secs: 0.0,
            ..Default::default()
        };
        let mut c = Committer::new(&asr, vec![], cfg);
        let second = vec![0.0; 16_000];
        c.push(&second).unwrap();
        c.push(&second).unwrap();
        assert_eq!(
            c.window_start, 0,
            "a single empty decode must not slide the window"
        );
        let committed = c.push(&second).unwrap();
        assert_eq!(join_words(&committed), "anyone gets");
    }

    #[test]
    fn finds_speech_no_word_covers() {
        let ws = |v: &[(&str, f32, f32)]| -> Vec<Word> {
            v.iter()
                .map(|&(t, s, e)| Word {
                    text: t.into(),
                    start: s,
                    end: e,
                })
                .collect()
        };
        // 10 s of speech, but the decodes only kept the last two seconds.
        let lost = ws(&[("what", 8.2, 8.5), ("is", 8.6, 8.8), ("left", 9.0, 9.4)]);
        assert_eq!(missed(&lost, &[(0.3, 10.0)], 1.5), [(0.0, 8.2)]);
        // Normal speech: words all the way through, nothing to rescue.
        let fine = ws(&[("a", 0.4, 1.0), ("b", 1.3, 2.4), ("c", 2.6, 3.9)]);
        assert!(missed(&fine, &[(0.3, 4.0)], 1.5).is_empty());
        // A whole phrase lost after a pause: bounded by the word before it.
        let gap = ws(&[("hi", 0.2, 0.6), ("end", 9.0, 9.5)]);
        assert_eq!(
            missed(&gap, &[(0.1, 0.7), (2.0, 6.0), (8.9, 9.6)], 1.5),
            [(1.5, 6.0)]
        );
    }

    #[test]
    fn a_rescue_adds_only_new_words() {
        let w = |t: &str| Word {
            text: t.into(),
            start: 0.0,
            end: 0.0,
        };
        let texts = |ws: Vec<Word>| join_words(&ws);
        // The word before heard again, then a hesitation: nothing new.
        assert_eq!(texts(new_words(vec![w("my,"), w("um,")], Some(&w("my")), None)), "");
        assert_eq!(
            texts(new_words(
                vec![w("my"), w("auth"), w("middleware"), w("no")],
                Some(&w("my")),
                Some(&w("no,"))
            )),
            "auth middleware"
        );
    }
}
