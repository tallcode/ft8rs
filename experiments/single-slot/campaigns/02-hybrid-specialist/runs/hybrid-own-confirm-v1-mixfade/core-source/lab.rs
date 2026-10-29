//! Independent research DSP. Native waveform, FEC and baseline calls are unchanged.
//! The first screen intentionally measures oracle synchronization to isolate demapping.
use rustfft::{num_complex::Complex64 as C, FftPlanner};
use std::f64::consts::TAU;
use std::time::Instant;

const FS: f64 = 12000.0;
const GRAY: [usize; 8] = [0, 1, 3, 2, 5, 6, 4, 7];
const COSTAS: [usize; 7] = [3, 1, 4, 0, 6, 5, 2];
const MESSAGES: [&str; 8] = ["CQ BG5ATV PM00", "K1JT BG5ATV -04", "K1JT BG5ATV R-07",
    "K1JT BG5ATV RR73", "K1JT BG5ATV 73", "CQ DL7XYZ JO62", "CQ VK3ABC QF22", "TNX BOB 73 GL"];

fn uniform(state: &mut u64) -> f64 {
    *state ^= *state >> 12; *state ^= *state << 25; *state ^= *state >> 27;
    (((state.wrapping_mul(2685821657736338717) >> 11) as f64) + 0.5) / (1u64 << 53) as f64
}
fn noise(seed: u64) -> Vec<f64> {
    let mut s = seed;
    let mut out = Vec::with_capacity(180000);
    for _ in 0..90000 {
        let r = (-2.0 * uniform(&mut s).ln()).sqrt(); let p = TAU * uniform(&mut s);
        out.push(r * p.cos()); out.push(r * p.sin());
    }
    out
}
fn carrier(message: &str, f: f64, dt: f64, snr: f64, phase: f64, drift: f64) -> Vec<f64> {
    let bits = crate::decode::packjt77::pack77(message);
    assert_eq!(crate::decode::packjt77::unpack77(&bits, None).as_deref(), Some(message));
    let tones = crate::decode::genft8::get_ft8_tones_from_77bits(&bits);
    let (re, im) = crate::decode::gen_ft8wave::gen_ft8wave(&tones, f);
    let power = re.iter().map(|v| v*v).sum::<f64>() / re.len() as f64;
    let amp = ((2500.0/6000.0) * 10f64.powf(snr/10.0) / power).sqrt();
    let mut out = vec![0.0;180000]; let start = ((dt+0.5)*FS).round() as isize;
    for i in 0..re.len() {
        let j = start + i as isize;
        if j >= 0 && j < out.len() as isize {
            let t=i as f64/FS; let p=phase+std::f64::consts::PI*drift*t*t;
            out[j as usize] = amp*(re[i]*p.cos()-im[i]*p.sin());
        }
    }
    out
}
fn add(x: &mut [f64], y: &[f64]) { for (a,b) in x.iter_mut().zip(y) { *a+=b; } }

struct Band { samples: Vec<C>, center: f64 }
impl Band {
    fn new(audio: &[f64], center: f64) -> Self {
        let mut planner=FftPlanner::<f64>::new();
        let mut fft: Vec<C> = audio.iter().map(|&x| C::new(x,0.0)).collect();
        planner.plan_fft_forward(180000).process(&mut fft);
        let bin=(center*15.0).round() as isize;
        let mut z=vec![C::new(0.0,0.0);3000];
        for (i,v) in z.iter_mut().enumerate() {
            let off=if i<1500 {i as isize} else {i as isize-3000};
            let src=bin+off;
            if src>0 && src<90000 { *v=fft[src as usize]/180000.0; }
        }
        planner.plan_fft_inverse(3000).process(&mut z);
        Self { samples:z, center:bin as f64/15.0 }
    }
    fn spectra(&self, f:f64, dt:f64, drift:f64) -> [[C;8];79] {
        let mut out=[[C::new(0.0,0.0);8];79]; let start=((dt+0.5)*200.0).round() as isize;
        for (k,row) in out.iter_mut().enumerate() {
            for n in 0..32 {
                let j=start+(32*k+n) as isize;
                if j<0 || j>=self.samples.len() as isize {continue;}
                let t=(j-start) as f64/200.0;
                let z=self.samples[j as usize]*C::from_polar(1.0,-TAU*((f-self.center)*t+0.5*drift*t*t));
                for (q,v) in row.iter_mut().enumerate() { *v+=z*C::from_polar(1.0,-TAU*q as f64*n as f64/32.0); }
            }
        }
        out
    }
}
fn median(mut x:Vec<f64>)->f64 {x.sort_by(f64::total_cmp); x[x.len()/2]}
fn variance(s:&[[C;8];79])->f64 {
    let mut v=Vec::new();
    for base in [0,36,72] { for k in 0..7 {for q in 0..8 {if q!=COSTAS[k] {v.push(s[base+k][q].norm_sqr());}}}}
    (median(v)/std::f64::consts::LN_2).max(1e-12)
}
fn log_i0(x:f64)->f64 {
    let x=x.abs();
    if x<3.75 {let y=(x/3.75).powi(2); (1.0+y*(3.5156229+y*(3.0899424+y*(1.2067492+y*(0.2659732+y*(0.0360768+y*0.0045813)))))).ln()}
    else {let y=3.75/x; x-0.5*x.ln()+(0.39894228+y*(0.01328592+y*(0.00225319+y*(-0.00157565+y*(0.00916281+y*(-0.02057706+y*(0.02635537+y*(-0.01647633+y*0.00392377)))))))).ln()}
}
fn lse(x:&[f64])->f64 {let m=x.iter().copied().fold(f64::NEG_INFINITY,f64::max); m+x.iter().map(|v|(v-m).exp()).sum::<f64>().ln()}
fn metrics(s:&[[C;8];79], block:usize, mode:&str)->Vec<f64> {
    let var=variance(s);
    let sync_power:f64=[0,36,72].iter().map(|&base|(0..7).map(|k|s[base+k][COSTAS[k]].norm_sqr()).sum::<f64>()).sum::<f64>()/21.0;
    let amp=(sync_power-var).max(0.1*var).sqrt();
    let mut llr=vec![0.0;174];
    for (half,base) in [7,43].iter().enumerate() {
        let mut k=0;
        while k<29 {
            let b=block.min(29-k); let count=8usize.pow(b as u32);
            let mut scores=vec![0.0;count];
            for (pattern,v) in scores.iter_mut().enumerate() {
                let mut z=C::new(0.0,0.0);
                for n in 0..b {let bits=(pattern>>(3*(b-1-n)))&7; z+=s[base+k+n][GRAY[bits]];}
                *v=match mode {"amp"=>z.norm(), "power"=>z.norm_sqr(), "lse"=>z.norm_sqr()/(b as f64*var), "bessel"=>log_i0(2.0*amp*z.norm()/var), _=>panic!("mode")};
            }
            for bit in 0..3*b {
                let mut a=Vec::new(); let mut c=Vec::new();
                for (p,&v) in scores.iter().enumerate() { if (p>>(3*b-1-bit))&1==1 {a.push(v)} else {c.push(v)} }
                let value=if mode=="amp"||mode=="power" {a.iter().copied().fold(f64::NEG_INFINITY,f64::max)-c.iter().copied().fold(f64::NEG_INFINITY,f64::max)} else {lse(&a)-lse(&c)};
                llr[half*87+k*3+bit]=value;
            }
            k+=b;
        }
    }
    if mode=="amp"||mode=="power" {
        let mean=llr.iter().sum::<f64>()/174.0;
        let sd=(llr.iter().map(|v|(v-mean).powi(2)).sum::<f64>()/174.0).sqrt().max(1e-12);
        for v in &mut llr {*v=*v/sd*2.83;}
    }
    llr
}
fn decode(llr:&[f64])->Option<String> {
    let result=crate::decode::decode174_91::decode174_91(llr,&[0;174],2)?;
    if result.cw.iter().all(|&x|x==0) {return None;}
    crate::decode::packjt77::unpack77(&result.message91[..77],None)
}
fn native(audio:&[f64])->Vec<String> {
    crate::decode::ft8_decode::decode_f64(audio,crate::DecodeOptions {
        nfa:Some(200.0),nfb:Some(3000.0),lft8apon:Some(false),ndepth:Some(3),..Default::default()
    }).into_iter().map(|m|m.msg).collect()
}

pub fn main() {
    let args:Vec<String>=std::env::args().collect();
    let trials:usize=args.get(1).map(|x|x.parse().unwrap()).unwrap_or(8);
    let root:u64=args.get(2).map(|x|x.parse().unwrap()).unwrap_or(2026102901);
    let scenario=args.get(3).map(String::as_str).unwrap_or("awgn");
    println!("scenario\tseed\tsnr\tmethod\tcorrect\twrong\tms\tmessage");
    for snr in [-18.0,-19.0,-20.0,-21.0,-22.0,-23.0,-24.0] {
        for trial in 0..trials {
            let seed=root+trial as u64*10007; let message=MESSAGES[trial%MESSAGES.len()];
            let f=1000.37;let dt=0.143;let drift=if scenario=="drift"{0.25}else{0.0};
            let mut audio=noise(seed);
            if scenario!="noise" && scenario!="blocker" {add(&mut audio,&carrier(message,f,dt,snr,0.27+trial as f64*0.73,drift));}
            if scenario=="mix"||scenario=="blocker" {add(&mut audio,&carrier("CQ K1ABC FN42",1025.0,0.05,-5.0,0.7,0.0));}
            if scenario=="cw" {for (i,v) in audio.iter_mut().enumerate(){*v+=0.45*(TAU*1012.9*i as f64/FS+0.4).cos();}}
            let t=Instant::now(); let rows=native(&audio);
            let correct=usize::from(rows.iter().any(|m|m==message)&&scenario!="noise"&&scenario!="blocker");
            let wrong=rows.iter().filter(|m|m.as_str()!=message&&m.as_str()!="CQ K1ABC FN42").count();
            println!("{scenario}\t{seed}\t{snr}\tnative\t{correct}\t{wrong}\t{:.3}\t{}",t.elapsed().as_secs_f64()*1000.0,rows.join(";"));
            let band=Band::new(&audio,1000.0);let s=band.spectra(f,dt,drift);
            for block in 1..=4 {for mode in ["amp","power","lse","bessel"] {
                let t=Instant::now();let row=decode(&metrics(&s,block,mode));
                let correct=usize::from(row.as_deref()==Some(message)&&scenario!="noise"&&scenario!="blocker");
                let wrong=usize::from(row.is_some()&&correct==0);
                println!("{scenario}\t{seed}\t{snr}\toracle-{block}-{mode}\t{correct}\t{wrong}\t{:.3}\t{}",t.elapsed().as_secs_f64()*1000.0,row.unwrap_or_default());
            }}
        }
    }
}
