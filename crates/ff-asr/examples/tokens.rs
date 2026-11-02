// Dumps raw tokens/timestamps for a WAV, to debug word grouping.
use sherpa_onnx::*;
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = &args[1];
    let wave = Wave::read(&args[2]).expect("wav");
    let mut c = OfflineRecognizerConfig::default();
    c.model_config.transducer = OfflineTransducerModelConfig {
        encoder: Some(format!("{dir}/encoder.int8.onnx")),
        decoder: Some(format!("{dir}/decoder.int8.onnx")),
        joiner: Some(format!("{dir}/joiner.int8.onnx")),
    };
    c.model_config.tokens = Some(format!("{dir}/tokens.txt"));
    c.model_config.model_type = Some("nemo_transducer".into());
    let r = OfflineRecognizer::create(&c).unwrap();
    let s = r.create_stream();
    s.accept_waveform(wave.sample_rate(), wave.samples());
    r.decode(&s);
    let res = s.get_result().unwrap();
    println!("TEXT: {:?}", res.text);
    for (i, t) in res.tokens.iter().enumerate() {
        println!(
            "{:?} {:?} {:?}",
            t,
            res.timestamps.as_ref().map(|v| v[i]),
            res.durations.as_ref().map(|v| v[i])
        );
    }
}
