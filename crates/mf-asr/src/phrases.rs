//! Streaming by phrase: VAD cuts the audio at pauses and each phrase is decoded
//! exactly once, as soon as the speaker pauses.
//!
//! Works with any [`Transcriber`], including models without usable word
//! timestamps (Whisper). Also immune to Whisper's habit of hallucinating text
//! on silence, because only VAD-confirmed speech is ever decoded.

use std::time::Instant;

use anyhow::Result;
use mf_audio::{SpeechSegment, SpeechSegmenter};

use crate::{Hypothesis, Session, SessionStats, Transcriber, Word, join_words};

const SAMPLE_RATE: f32 = 16_000.0;
/// Real audio kept before and after each VAD phrase. VAD flags speech a little late,
/// and Whisper drops a clipped first word.
const PRE_ROLL_SECS: f32 = 0.3;
const POST_ROLL_SECS: f32 = 0.3;

pub struct PhraseSession<'a> {
    asr: &'a dyn Transcriber,
    hotwords: Vec<String>,
    segmenter: SpeechSegmenter,
    /// Phrases that ended after key-up, decoded in `finish`.
    queued: Vec<SpeechSegment>,
    words: Vec<Word>,
    /// Everything heard so far, so phrases can be decoded with pre- and post-roll.
    audio: Vec<f32>,
    stats: SessionStats,
}

impl<'a> PhraseSession<'a> {
    pub fn new(
        asr: &'a dyn Transcriber,
        hotwords: Vec<String>,
        segmenter: SpeechSegmenter,
    ) -> Self {
        Self {
            asr,
            hotwords,
            segmenter,
            queued: Vec::new(),
            words: Vec::new(),
            audio: Vec::with_capacity(16_000 * 60),
            stats: SessionStats::default(),
        }
    }

    fn decode(&mut self, seg: &SpeechSegment) -> Result<Vec<Word>> {
        let from = seg
            .start
            .saturating_sub((PRE_ROLL_SECS * SAMPLE_RATE) as usize);
        let to = (seg.start + seg.samples.len() + (POST_ROLL_SECS * SAMPLE_RATE) as usize)
            .min(self.audio.len());
        let pcm = &self.audio[from..to.max(from)];

        let t = Instant::now();
        let hyp = self.asr.transcribe(pcm, &self.hotwords)?;
        self.stats.decodes += 1;
        self.stats.decode_time += t.elapsed();

        let offset = from as f32 / SAMPLE_RATE;
        Ok(hyp
            .words
            .into_iter()
            .map(|w| Word {
                start: w.start + offset,
                end: w.end + offset,
                ..w
            })
            .collect())
    }
}

impl Session for PhraseSession<'_> {
    fn push(&mut self, pcm: &[f32]) -> Result<Vec<Word>> {
        self.audio.extend_from_slice(pcm);
        let mut newly = Vec::new();
        for seg in self.segmenter.push(pcm) {
            newly.extend(self.decode(&seg)?);
        }
        self.words.extend(newly.iter().cloned());
        Ok(newly)
    }

    fn append(&mut self, pcm: &[f32]) {
        self.audio.extend_from_slice(pcm);
        let segs = self.segmenter.push(pcm);
        self.queued.extend(segs);
    }

    fn partial_text(&self) -> (String, String) {
        (join_words(&self.words), String::new())
    }

    fn audio_secs(&self) -> f32 {
        self.audio.len() as f32 / SAMPLE_RATE
    }

    fn finish(mut self: Box<Self>) -> Result<(Hypothesis, SessionStats)> {
        let t = Instant::now();
        let mut rest = std::mem::take(&mut self.queued);
        rest.extend(self.segmenter.flush());
        self.stats.tail_secs =
            rest.iter().map(|s| s.samples.len()).sum::<usize>() as f32 / SAMPLE_RATE;
        for seg in &rest {
            let words = self.decode(seg)?;
            self.words.extend(words);
        }
        self.stats.tail_time = t.elapsed();
        let stats = self.stats.clone();
        Ok((
            Hypothesis {
                words: std::mem::take(&mut self.words),
            },
            stats,
        ))
    }
}
