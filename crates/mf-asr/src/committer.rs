//! Streaming on top of an offline transcriber (LocalAgreement-2).
//!
//! Every `step` of new audio we re-decode the *uncommitted window*. Words that
//! two consecutive decodes agree on (ignoring case and punctuation) are
//! committed and the window slides past them. On key-up, [`Committer::finish`]
//! only has to decode the short uncommitted tail, which is what keeps
//! key-up → text latency low regardless of how long the user spoke.

use std::time::Instant;

use anyhow::Result;

use crate::{Hypothesis, Session, SessionStats, Transcriber, Word, join_words};

const SAMPLE_RATE: f32 = 16_000.0;
/// Audio kept in the window when a decode finds no words.
const SILENCE_KEEP_SECS: f32 = 0.5;

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
            context_secs: 1.0,
        }
    }
}

pub struct Committer<'a> {
    asr: &'a dyn Transcriber,
    hotwords: Vec<String>,
    cfg: CommitterConfig,
    audio: Vec<f32>,
    /// Start of the audio still being decoded. Silence moves it forward too.
    window_start: usize,
    /// End of the audio whose words are committed. Only words before this are discarded
    /// from re-decoded context; audio skipped as silence may still hold a word onset.
    committed_end: usize,
    last_decode_at: usize,
    committed: Vec<Word>,
    pending: Vec<Word>,
    pub stats: SessionStats,
}

impl<'a> Committer<'a> {
    pub fn new(asr: &'a dyn Transcriber, hotwords: Vec<String>, cfg: CommitterConfig) -> Self {
        Self {
            asr,
            hotwords,
            cfg,
            audio: Vec::with_capacity(16_000 * 60),
            window_start: 0,
            committed_end: 0,
            last_decode_at: 0,
            committed: Vec::new(),
            pending: Vec::new(),
            stats: SessionStats::default(),
        }
    }

    fn step(&mut self) -> Result<Vec<Word>> {
        self.last_decode_at = self.audio.len();
        let hyp = self.decode_window()?;

        if hyp.is_empty() {
            // Silence (or noise the model ignores): nothing to wait for, so don't keep
            // re-decoding it. Leave a little audio in case a word is just starting.
            let keep = (SILENCE_KEEP_SECS * SAMPLE_RATE) as usize;
            self.window_start = self.window_start.max(self.audio.len().saturating_sub(keep));
            self.pending.clear();
            return Ok(Vec::new());
        }

        let mut agreed = common_prefix(&self.pending, &hyp);
        let window_secs = (self.audio.len() - self.window_start) as f32 / SAMPLE_RATE;
        if agreed == 0 && window_secs > self.cfg.max_window_secs {
            // Long unstable stretch (noise, mumbling): don't let the tail grow without bound.
            let horizon = self.audio_secs() - self.cfg.keep_secs;
            agreed = hyp.iter().take_while(|w| w.end <= horizon).count();
        }
        if agreed == 0 {
            self.pending = hyp;
            return Ok(Vec::new());
        }

        let newly: Vec<Word> = hyp[..agreed].to_vec();
        let last_end = newly.last().map(|w| w.end).unwrap_or(0.0);
        // Cut halfway into the gap before the next word so neither word loses audio.
        let cut = match hyp.get(agreed) {
            Some(next) => (last_end + next.start) / 2.0,
            None => last_end,
        };
        let cut =
            ((cut.max(0.0) * SAMPLE_RATE) as usize).clamp(self.committed_end, self.audio.len());
        self.committed_end = cut;
        self.window_start = self.window_start.max(cut);
        self.pending = hyp[agreed..].to_vec();
        self.committed.extend(newly.iter().cloned());
        Ok(newly)
    }

    /// Decodes the uncommitted window (plus left context) and returns only the
    /// new words, with absolute timestamps.
    fn decode_window(&mut self) -> Result<Vec<Word>> {
        let ctx = (self.cfg.context_secs * SAMPLE_RATE) as usize;
        let from = self.window_start.saturating_sub(ctx);
        let offset = from as f32 / SAMPLE_RATE;
        let boundary = self.committed_end as f32 / SAMPLE_RATE;

        let t = Instant::now();
        let hyp = self.asr.transcribe(&self.audio[from..], &self.hotwords)?;
        self.stats.decodes += 1;
        self.stats.decode_time += t.elapsed();

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
        self.audio.extend_from_slice(pcm);
    }

    fn push(&mut self, pcm: &[f32]) -> Result<Vec<Word>> {
        self.audio.extend_from_slice(pcm);
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
        self.stats.tail_time = t.elapsed();
        self.committed.extend(hyp);
        let stats = self.stats.clone();
        Ok((
            Hypothesis {
                words: std::mem::take(&mut self.committed),
            },
            stats,
        ))
    }
}

fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn common_prefix(a: &[Word], b: &[Word]) -> usize {
    a.iter()
        .zip(b)
        .take_while(|(x, y)| norm(&x.text) == norm(&y.text))
        .count()
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
}
