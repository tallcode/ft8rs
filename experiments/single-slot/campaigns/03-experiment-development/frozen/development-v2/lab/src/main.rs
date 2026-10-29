//! Isolated truth generator and serial product-profile measurement only.
//! All receiver calls are blind; truth coordinates are used solely by grading.
use ft8rs::lab_bridge as bridge;
use ft8rs::{DecodeProfile, SlotTimestamp, StreamDecodeConfig, StreamDecodedMessage};
use ft8rs::stream::ProfileStreamDecodeSession;
use std::f64::consts::TAU;
use std::time::Instant;
const FS:f64=12000.0;
fn uniform(state: &mut u64) -> f64 {
    *state ^= *state >> 12;
    *state ^= *state << 25;
    *state ^= *state >> 27;
    (((state.wrapping_mul(2685821657736338717) >> 11) as f64) + 0.5) / (1u64 << 53) as f64
}
fn noise(seed: u64) -> Vec<f64> {
    let mut s = seed;
    let mut out = Vec::with_capacity(180000);
    for _ in 0..90000 {
        let r = (-2.0 * uniform(&mut s).ln()).sqrt();
        let p = TAU * uniform(&mut s);
        out.push(r * p.cos());
        out.push(r * p.sin());
    }
    out
}
fn fresh_message(seed: u64, trial: usize) -> String {
    let mut s = seed ^ 0x8a0f51a6d43c9217;
    let mut call = || {
        let prefixes = [
            "K", "N", "W", "DL", "VK", "JA", "G", "F", "EA", "PA", "OH", "LU",
        ];
        let prefix = prefixes[(uniform(&mut s) * prefixes.len() as f64) as usize];
        let digit = (uniform(&mut s) * 10.0) as u8;
        let letters: String = (0..3)
            .map(|_| (b'A' + (uniform(&mut s) * 26.0) as u8) as char)
            .collect();
        format!("{prefix}{digit}{letters}")
    };
    let a = call();
    let b = call();
    let grid = format!(
        "{}{}{}{}",
        (b'A' + (uniform(&mut s) * 18.0) as u8) as char,
        (b'A' + (uniform(&mut s) * 18.0) as u8) as char,
        (uniform(&mut s) * 10.0) as u8,
        (uniform(&mut s) * 10.0) as u8
    );
    match trial % 6 {
        0 => format!("CQ {a} {grid}"),
        1 => format!("{a} {b} -{:02}", 1 + trial % 29),
        2 => format!("{a} {b} R-{:02}", 1 + trial % 29),
        3 => format!("{a} {b} RR73"),
        4 => format!("{a} {b} 73"),
        _ => format!("{a} {b} {grid}"),
    }
}
fn carrier(message: &str, f: f64, dt: f64, snr: f64, phase: f64, drift: f64) -> Vec<f64> {
    let bits = bridge::pack(message);
    assert_eq!(bridge::unpack(&bits, None).as_deref(), Some(message));
    let tones = bridge::tones(&bits);
    let (re, im) = bridge::wave(&tones, f);
    let power = re.iter().map(|v| v * v).sum::<f64>() / re.len() as f64;
    let amp = ((2500.0 / 6000.0) * 10f64.powf(snr / 10.0) / power).sqrt();
    let mut out = vec![0.0; 180000];
    let start = ((dt + 0.5) * FS).round() as isize;
    for i in 0..re.len() {
        let j = start + i as isize;
        if j >= 0 && j < out.len() as isize {
            let t = i as f64 / FS;
            let p = phase + std::f64::consts::PI * drift * t * t;
            out[j as usize] = amp * (re[i] * p.cos() - im[i] * p.sin());
        }
    }
    out
}
fn add(x: &mut [f64], y: &[f64]) {
    for (a, b) in x.iter_mut().zip(y) {
        *a += b;
    }
}

#[derive(Clone)]
struct Truth { msg:String, f:f64, dt:f64, target:bool }
fn matches(row:&StreamDecodedMessage, truth:&Truth)->bool {
    row.msg==truth.msg && (row.freq-truth.f).abs()<=5.0 && (row.dt-truth.dt).abs()<=0.3
}
fn measure(samples:&[f32], truth:&[Truth], methods:&str) {
    let ts=SlotTimestamp::parse("261029_120000").unwrap();
    for method in methods.split(',') {
        let config=StreamDecodeConfig {
            profile:match method { "wsjtx"=>DecodeProfile::Wsjtx,"jtdx"=>DecodeProfile::Jtdx,_=>DecodeProfile::Hybrid },
            hybrid_experiment:method=="exp", cooperative_realtime:false,
            ..Default::default()
        };
        assert!(["wsjtx","jtdx","hybrid","exp"].contains(&method));
        let mut decoder=ProfileStreamDecodeSession::new(config);
        let start=Instant::now();
        let mut callback_rows=Vec::new();
        let rows=decoder.decode_slot_streaming_with_provenance_at(&ts,samples,|tag| {
            let r=&tag.decode;
            println!("ROW\t{method}\t{}\t{:.12}\t{:.12}\t{:.12}\t{:.12}\t{:?}\t{:?}\t{:.3}",
                     r.msg,r.freq,r.dt,r.snr,r.sync,r.itone,tag.provenance,start.elapsed().as_secs_f64()*1000.0);
            callback_rows.push(r.clone()); Ok(())
        }).unwrap();
        let ms=start.elapsed().as_secs_f64()*1000.0;
        assert_eq!(rows.len(),callback_rows.len());
        let correct=usize::from(rows.iter().any(|r| truth.iter().any(|t|t.target&&matches(r,t))));
        let wrong=rows.iter().filter(|r|!truth.iter().any(|t|matches(r,t))).count();
        println!("END\t{method}\t{correct}\t{wrong}\t{ms:.3}\t{}",rows.len());
        if let ProfileStreamDecodeSession::Hybrid(ref hybrid)=decoder {
            eprintln!("DIAG\t{method}\t{:?}",hybrid.last_diagnostics());
        }
    }
}
fn main() {
    let args:Vec<String>=std::env::args().collect();
    // file path slot methods: no truth claims for unlabelled recording rows.
    if args[1]=="file" {
        let wav=ft8rs::input::audio::read_wav_mono_f32(&args[2]).unwrap();
        let samples=ft8rs::input::audio::downsample_12k(&wav.samples,wav.sample_rate);
        let slot:usize=args[3].parse().unwrap();
        let mut audio:Vec<f32>=samples.into_iter().skip(slot*180000).take(180000).collect();
        audio.resize(180000,0.0);
        measure(&audio,&[],&args[4]); return;
    }
    let scenario=&args[1];
    let seed:u64=args[2].parse().unwrap();
    assert_ne!(seed,0);
    let trial:usize=args[3].parse().unwrap();
    let snr:f64=args[4].parse().unwrap();
    let message=fresh_message(seed,trial);
    let mut coordinates=seed.wrapping_add(832719);
    let f=250.0+uniform(&mut coordinates)*2650.0;
    let dt=-0.25+uniform(&mut coordinates)*1.25;
    let mut audio=noise(seed);
    let mut truth=Vec::new();
    if !["noise","cwonly","blocker","mix2only","pilotonly"].contains(&scenario.as_str()) {
        let drift=if scenario=="drift" {0.25} else {0.0};
        add(&mut audio,&carrier(&message,f,dt,snr,0.27+trial as f64*0.73,drift));
        truth.push(Truth {msg:message.clone(),f,dt,target:true});
    }
    if ["mix","blocker","mix2","mix2only","mixfade"].contains(&scenario.as_str()) {
        let drift=if scenario=="mixfade" {0.18} else {0.0};
        let mut blocker=carrier("CQ K1ABC FN42",f+24.63,dt-0.093,-5.0,0.7,drift);
        if scenario=="mixfade" {for (i,x) in blocker.iter_mut().enumerate() {*x*=0.3+0.7*(0.5+0.5*(TAU*0.5*i as f64/FS).cos());}}
        add(&mut audio,&blocker);
        truth.push(Truth {msg:"CQ K1ABC FN42".into(),f:f+24.63,dt:dt-0.093,target:false});
    }
    if ["mix2","mix2only"].contains(&scenario.as_str()) {
        add(&mut audio,&carrier("CQ W9XYZ EN50",f-31.17,dt+0.061,-9.0,1.3,0.0));
        truth.push(Truth {msg:"CQ W9XYZ EN50".into(),f:f-31.17,dt:dt+0.061,target:false});
    }
    if ["cw","cwonly"].contains(&scenario.as_str()) {
        for (i,x) in audio.iter_mut().enumerate() {*x+=0.45*(TAU*(f+12.53)*i as f64/FS+0.4).cos();}
    }
    // Public Costas only: intentionally no transmitted LDPC payload.
    if scenario=="pilotonly" {
        let mut pilots=carrier(&message,f,dt,-5.0,0.27,0.0);
        let start=((dt+0.5)*FS).round() as isize;
        for (j,x) in pilots.iter_mut().enumerate() {
            let i=j as isize-start;
            let sym=if i>=0 {i/1920} else {-1};
            if !(0..=6).contains(&sym) && !(36..=42).contains(&sym) && !(72..=78).contains(&sym) {*x=0.0;}
        }
        add(&mut audio,&pilots);
    }
    for t in &truth {println!("TRUTH\t{}\t{:.12}\t{:.12}\t{}",t.msg,t.f,t.dt,t.target);}
    let samples:Vec<f32>=audio.into_iter().map(|x|x as f32).collect();
    if let Ok(path)=std::env::var("FT8RS_MEASURE_WAV") {
        assert!(!std::path::Path::new(&path).exists());
        let mut writer=hound::WavWriter::create(path,hound::WavSpec {channels:1,sample_rate:12000,bits_per_sample:32,sample_format:hound::SampleFormat::Float}).unwrap();
        for &x in &samples {writer.write_sample(x).unwrap();} writer.finalize().unwrap();
    }
    measure(&samples,&truth,&args[5]);
}
