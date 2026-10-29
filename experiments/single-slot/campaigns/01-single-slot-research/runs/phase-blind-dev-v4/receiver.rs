//! Independent research DSP. Native waveform, FEC and baseline calls are unchanged.
//! The first screen intentionally measures oracle synchronization to isolate demapping.
use ft8rs::lab_bridge as bridge;
use rustfft::{num_complex::Complex64 as C, FftPlanner};
use std::f64::consts::TAU;
use std::time::Instant;
use std::sync::OnceLock;

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
    let bits = bridge::pack(message);
    assert_eq!(bridge::unpack(&bits, None).as_deref(), Some(message));
    let tones = bridge::tones(&bits);
    let (re, im) = bridge::wave(&tones, f);
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
fn basis() -> &'static [[C;32];8] {
    static B:OnceLock<[[C;32];8]>=OnceLock::new();
    B.get_or_init(||std::array::from_fn(|q|std::array::from_fn(|n|C::from_polar(1.0,-TAU*q as f64*n as f64/32.0))))
}
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
                for (q,v) in row.iter_mut().enumerate() { *v+=z*basis()[q][n]; }
            }
        }
        out
    }
    fn sync_stats(&self, f:f64, dt:f64, drift:f64)->(f64,f64) {
        let start=((dt+0.5)*200.0).round() as isize;
        let mut power=0.0; let mut total=0.0; let mut count=0;let mut coherent=C::new(0.0,0.0);
        for base in [0,36,72] {for (k,&tone) in COSTAS.iter().enumerate() {
            let j0=start+32*(base+k) as isize;
            if j0<0 || j0+31>=self.samples.len() as isize {continue;}
            let mut z=C::new(0.0,0.0);let mut en=0.0;
            let t0=(j0-start) as f64/200.0;let df=f-self.center;
            let mut osc=C::from_polar(1.0,-TAU*(df*t0+0.5*drift*t0*t0));
            let mut step=C::from_polar(1.0,-TAU*((df+drift*t0)/200.0+0.5*drift/40000.0));
            let accel=C::from_polar(1.0,-TAU*drift/40000.0);
            for n in 0..32 {
                let j=j0+n as isize;let t=(j-start) as f64/200.0;
                let sample=self.samples[j as usize];
                let _=t;
                z+=sample*basis()[tone][n]*osc;osc*=step;step*=accel;
                en+=sample.norm_sqr();
            }
            power+=z.norm_sqr();coherent+=z; total+=en;count+=1;
        }}
        if count<14 {return (0.0,0.0);}
        (power/total.max(1e-15),coherent.norm_sqr()/total.max(1e-15))
    }
    fn sync_score(&self,f:f64,dt:f64,drift:f64)->f64 {self.sync_stats(f,dt,drift).0}
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
    let varq:[f64;8]=std::array::from_fn(|q| (median(s.iter().map(|r|r[q].norm_sqr()).collect())/std::f64::consts::LN_2).max(var*0.6));
    let sync_power:f64=[0,36,72].iter().map(|&base|(0..7).map(|k|s[base+k][COSTAS[k]].norm_sqr()).sum::<f64>()).sum::<f64>()/21.0;
    let robust_power=median([0,36,72].iter().flat_map(|&base|(0..7).map(move |k|s[base+k][COSTAS[k]].norm_sqr())).collect());
    let amp=(if mode=="robust" {robust_power-var} else {sync_power-var}).max(0.1*var).sqrt();
    let mut llr=vec![0.0;174];
    for (half,base) in [7,43].iter().enumerate() {
        let mut k=0;
        while k<29 {
            let b=block.min(29-k); let count=8usize.pow(b as u32);
            let mut scores=vec![0.0;count];
            for (pattern,v) in scores.iter_mut().enumerate() {
                let mut z=C::new(0.0,0.0);let mut invsum=0.0;
                for n in 0..b {let bits=(pattern>>(3*(b-1-n)))&7;let q=GRAY[bits];
                    let weight=if mode=="robust" {1.0/varq[q]} else {1.0};
                    z+=s[base+k+n][q]*weight;invsum+=weight;
                }
                *v=match mode {"amp"=>z.norm(), "power"=>z.norm_sqr(), "lse"=>z.norm_sqr()/(b as f64*var), "bessel"=>log_i0(2.0*amp*z.norm()/var), "robust"=>log_i0(2.0*amp*z.norm())-amp*amp*invsum, _=>panic!("mode")};
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
    let result=bridge::fec(llr,&[0;174],2)?;
    if result.cw.iter().all(|&x|x==0) {return None;}
    bridge::unpack(&result.message91[..77],None)
}
fn coherent_metrics(s:&[[C;8];79], scheme:usize)->Vec<f64> {
    let var=variance(s);
    let h:[C;3]=std::array::from_fn(|g|(0..7).map(|k|s[[0,36,72][g]+k][COSTAS[k]]).sum::<C>()/7.0);
    let global=(h[0]+h[1]+h[2])/3.0;
    let mut llr=Vec::with_capacity(174);
    for base in [7,43] {for k in base..base+29 {
        let channel=match scheme {
            0=>global,
            1=>h[if k<21 {0}else if k<57 {1}else{2}],
            _=>{let x=((k as f64-3.0)/36.0).clamp(0.0,2.0);let lo=(x.floor() as usize).min(1);h[lo]*(1.0-(x-lo as f64))+h[lo+1]*(x-lo as f64)},
        };
        let scores:[f64;8]=std::array::from_fn(|q|2.0*(s[k][GRAY[q]]*channel.conj()).re/var);
        for bit in (0..3).rev() {
            let one:Vec<f64>=scores.iter().enumerate().filter(|(q,_)|(q>>bit)&1==1).map(|(_,v)|*v).collect();
            let zero:Vec<f64>=scores.iter().enumerate().filter(|(q,_)|(q>>bit)&1==0).map(|(_,v)|*v).collect();
            llr.push(lse(&one)-lse(&zero));
        }
    }}
    llr
}
fn native(audio:&[f64])->Vec<String> {
    bridge::baseline(audio,ft8rs::DecodeOptions {
        nfa:Some(200.0),nfb:Some(3000.0),lft8apon:Some(false),ndepth:Some(3),..Default::default()
    }).into_iter().map(|m|m.msg).collect()
}

#[derive(Clone)]
struct Candidate { f:f64, dt:f64, score:f64, band:usize }
#[derive(Clone)]
struct Row { msg:String, f:f64, dt:f64, method:String }
fn discover(audio:&[f64])->(Vec<Band>,Vec<Candidate>) {
    let mut bands=Vec::new();let mut candidates=Vec::new();
    let mut planner=FftPlanner::<f64>::new();let fft=planner.plan_fft_forward(64);
    for bi in 0..28 {
        let band=Band::new(audio,250.0+100.0*bi as f64);
        let mut powers=vec![[0.0;64];372];
        for (step,row) in powers.iter_mut().enumerate() {
            let mut z=vec![C::new(0.0,0.0);64];
            for n in 0..32 {let j=step*8+n;if j<3000 {z[n]=band.samples[j];}}
            fft.process(&mut z);
            for q in 0..64 {row[q]=z[q].norm_sqr();}
        }
        // Each candidate gets its best coarse time; no target frequency/message is supplied.
        for fq in -16isize..=16 {
            let f=band.center+fq as f64*3.125;
            if !(200.0..=3000.0).contains(&f) {continue;}
            let mut best=(0.0,0.0);
            for lag in -50isize..=62 {
                let mut good=0.0;let mut off=0.0;let mut count=0;
                for base in [0isize,36,72] {for k in 0..7 {
                    let step=lag+4*(base+k as isize);
                    if step<0||step>=powers.len() as isize {continue;}
                    let p=&powers[step as usize];
                    good+=p[(fq+2*COSTAS[k] as isize).rem_euclid(64) as usize];
                    for q in 0..8 {if q!=COSTAS[k] {off+=p[(fq+2*q as isize).rem_euclid(64) as usize];}}
                    count+=1;
                }}
                if count<14 {continue;}
                let score=7.0*good/off.max(1e-15);
                if score>best.0 {best=(score,lag as f64*0.04-0.5);}
            }
            if best.0>1.5 {candidates.push(Candidate{f,dt:best.1,score:best.0,band:bi});}
        }
        bands.push(band);
    }
    candidates.sort_by(|a,b|b.score.total_cmp(&a.score));
    let mut chosen:Vec<Candidate>=Vec::new();
    for c in candidates {
        if chosen.iter().any(|x|(x.f-c.f).abs()<4.0&&(x.dt-c.dt).abs()<0.08) {continue;}
        chosen.push(c);if chosen.len()==96 {break;}
    }
    (bands,chosen)
}
fn refine(band:&Band,c:&Candidate)->Vec<Candidate> {
    let mut grid=Vec::new();
    for df in -3..=3 {for dt in -4..=4 {
        let f=c.f+df as f64*0.5;let t=c.dt+dt as f64*0.01;
        grid.push(Candidate{f,dt:t,score:band.sync_score(f,t,0.0),band:c.band});
    }}
    grid.sort_by(|a,b|b.score.total_cmp(&a.score));
    let best=grid[0].clone();
    for df in -1..=1 {for dt in -1..=1 {
        let f=best.f+df as f64*0.125;let t=best.dt+dt as f64*0.005;
        grid.push(Candidate{f,dt:t,score:band.sync_score(f,t,0.0),band:c.band});
    }}
    grid.sort_by(|a,b|b.score.total_cmp(&a.score));
    let mut out:Vec<Candidate>=Vec::new();
    for c in grid {if !out.iter().any(|p|(p.f-c.f).abs()<0.2&&(p.dt-c.dt).abs()<0.009) {out.push(c);}if out.len()==4 {break;}}
    out
}
fn refine_coherent(band:&Band,c:&Candidate)->Vec<Candidate> {
    let mut grid=Vec::new();
    for df in -30..=30 {for dt in -2..=2 {
        let f=c.f+df as f64*0.025;let t=c.dt+dt as f64*0.005;
        grid.push(Candidate{f,dt:t,score:band.sync_stats(f,t,0.0).1,band:c.band});
    }}
    grid.sort_by(|a,b|b.score.total_cmp(&a.score));
    let mut out:Vec<Candidate>=Vec::new();
    for c in grid {if !out.iter().any(|p|(p.f-c.f).abs()<0.05&&(p.dt-c.dt).abs()<0.009) {out.push(c);}if out.len()==4 {break;}}
    out
}
fn search(audio:&[f64], joint:bool, phase:bool)->Vec<Row> {
    let (bands,candidates)=discover(audio);let mut rows:Vec<Row>=Vec::new();
    for (ci,c) in candidates.into_iter().enumerate() {
        let band=&bands[c.band];let hypotheses=refine(band,&c);
        for (hi,h) in hypotheses.iter().enumerate() {
            if !joint&&hi>0 {break;}
            let s=band.spectra(h.f,h.dt,0.0);
            for (block,mode) in [(1,"amp"),(3,"amp"),(4,"bessel"),(3,"robust")] {
                if !joint&&(block==4||mode=="robust") {continue;}
                if let Some(msg)=decode(&metrics(&s,block,mode)) {
                    if !rows.iter().any(|r|r.msg==msg&&(r.f-h.f).abs()<5.0&&(r.dt-h.dt).abs()<0.3) {
                        rows.push(Row{msg,f:h.f,dt:h.dt,method:format!("h{hi}-{block}-{mode}")});
                    }
                    break;
                }
            }
            if rows.iter().any(|r|(r.f-h.f).abs()<3.0&&(r.dt-h.dt).abs()<0.2) {break;}
        }
        if phase && ci<32 && !rows.iter().any(|r|(r.f-c.f).abs()<3.0&&(r.dt-c.dt).abs()<0.2) {
            for (hi,h) in refine_coherent(band,&hypotheses[0]).iter().enumerate() {
                let s=band.spectra(h.f,h.dt,0.0);
                if let Some(msg)=decode(&coherent_metrics(&s,0)) {
                    if !rows.iter().any(|r|r.msg==msg&&(r.f-h.f).abs()<5.0&&(r.dt-h.dt).abs()<0.3) {
                        rows.push(Row{msg,f:h.f,dt:h.dt,method:format!("phase-{hi}")});
                    }
                    break;
                }
            }
        }
    }
    rows
}

fn fit_message(audio:&[f64],row:&Row)->Option<(f64,f64,f64)> {
    let bits=bridge::pack(&row.msg);
    if bridge::unpack(&bits,None).as_deref()!=Some(row.msg.as_str()) {return None;}
    let tones=bridge::tones(&bits);let (re,im)=bridge::wave(&tones,0.0);
    let template:Vec<C>=(0..2528).map(|n|C::new(re[n*60],im[n*60])).collect();
    let en=template.iter().map(C::norm_sqr).sum::<f64>();
    let band=Band::new(audio,row.f+22.0);
    let evaluate=|f:f64,dt:f64|->f64 {
        let start=(dt+0.5)*200.0;let rotation=C::from_polar(1.0,TAU*(f-band.center)/200.0);
        let mut phase=C::new(1.0,0.0);let mut z=C::new(0.0,0.0);
        for (n,&v) in template.iter().enumerate() {
            let index=start+n as f64;let j=index.floor() as isize;let frac=index-j as f64;
            if j>=0&&j+1<band.samples.len() as isize {
                let y=band.samples[j as usize]*(1.0-frac)+band.samples[j as usize+1]*frac;
                z+=y*(v*phase).conj();
            }
            phase*=rotation;
        }
        z.norm_sqr()/en
    };
    let mut best=(0.0,row.f,row.dt);
    for df in -40..=40 {for dt in -4..=4 {
        let f=row.f+df as f64*0.025;let t=row.dt+dt as f64*0.005;let score=evaluate(f,t);
        if score>best.0 {best=(score,f,t);}
    }}
    let anchor=best;
    for df in -3..=3 {for dt in -2..=2 {
        let f=anchor.1+df as f64*0.005;let t=anchor.2+dt as f64*0.001;let score=evaluate(f,t);
        if score>best.0 {best=(score,f,t);}
    }}
    let n0=variance(&band.spectra(best.1,best.2,0.0))/32.0;
    Some((best.1,best.2,best.0/n0))
}
fn cancel_constant(audio:&[f64],row:&Row,f:f64,dt:f64)->Vec<f64> {
    let tones=bridge::tones(&bridge::pack(&row.msg));let (re,im)=bridge::wave(&tones,f);
    let start=((dt+0.5)*FS).round() as isize;
    let mut aa=0.0;let mut bb=0.0;let mut ab=0.0;let mut ya=0.0;let mut yb=0.0;
    for i in 0..re.len() {let j=start+i as isize;if j>=0&&j<audio.len() as isize {
        aa+=re[i]*re[i];bb+=im[i]*im[i];ab+=re[i]*im[i];ya+=audio[j as usize]*re[i];yb+=audio[j as usize]*im[i];
    }}
    let det=(aa*bb-ab*ab).max(1e-15);let a=(ya*bb-yb*ab)/det;let b=(yb*aa-ya*ab)/det;
    let mut out=audio.to_vec();
    for i in 0..re.len(){let j=start+i as isize;if j>=0&&j<out.len() as isize {out[j as usize]-=a*re[i]+b*im[i];}}
    out
}
fn search_cancel(audio:&[f64],model:&str)->Vec<Row> {
    let mut rows=search(audio,true,true);
    let mut seeds=Vec::new();
    for row in &rows {if let Some((f,dt,score))=fit_message(audio,row) {if score>40.0 {seeds.push((score,row.clone(),f,dt));}}}
    seeds.sort_by(|a,b|b.0.total_cmp(&a.0));seeds.truncate(3);
    if seeds.is_empty(){return rows;}
    let mut residual=audio.to_vec();
    for (score,row,f,dt) in seeds {
        eprintln!("FIT\t{}\t{:.6}\t{:.6}\t{score:.3}",row.msg,f,dt);
        if model=="native" {bridge::subtract(&mut residual,&bridge::tones(&bridge::pack(&row.msg)),f,dt+0.5,true);}
        else {residual=cancel_constant(&residual,&row,f,dt);}
    }
    for mut row in search(&residual,true,true) {
        if !rows.iter().any(|r|r.msg==row.msg&&(r.f-row.f).abs()<5.0&&(r.dt-row.dt).abs()<0.3) {
            row.method=format!("sic-{}",row.method);rows.push(row);
        }
    }
    rows
}

pub fn main() {
    let args:Vec<String>=std::env::args().collect();
    let trials:usize=args.get(1).map(|x|x.parse().unwrap()).unwrap_or(8);
    let root:u64=args.get(2).map(|x|x.parse().unwrap()).unwrap_or(2026102901);
    let scenario_arg=args.get(3).map(String::as_str).unwrap_or("awgn");
    let sic=scenario_arg.starts_with("sic-");
    let blind=scenario_arg.starts_with("blind-")||sic;
    let scenario=scenario_arg.strip_prefix("blind-").or_else(||scenario_arg.strip_prefix("sic-")).unwrap_or(scenario_arg);
    println!("scenario\tseed\tsnr\tmethod\tcorrect\twrong\tms\tmessage");
    let levels=if scenario=="noise"||scenario=="blocker" {vec![-24.0]} else {vec![-18.0,-19.0,-20.0,-21.0,-22.0,-23.0,-24.0]};
    for snr in levels {
        for trial in 0..trials {
            let seed=root+trial as u64*10007; let message=MESSAGES[trial%MESSAGES.len()];
            let f=1000.37;let dt=0.143;let drift=if scenario=="drift"{0.25}else{0.0};
            let mut audio=noise(seed);
            if scenario!="noise" && scenario!="blocker" {add(&mut audio,&carrier(message,f,dt,snr,0.27+trial as f64*0.73,drift));}
            if scenario=="mix"||scenario=="blocker" {add(&mut audio,&carrier("CQ K1ABC FN42",1025.0,0.05,-5.0,0.7,0.0));}
            if scenario=="cw" {for (i,v) in audio.iter_mut().enumerate(){*v+=0.45*(TAU*1012.9*i as f64/FS+0.4).cos();}}
            let t=Instant::now(); let rows=native(&audio);
            let correct=usize::from(rows.iter().any(|m|m==message)&&scenario!="noise"&&scenario!="blocker");
            let wrong=rows.iter().filter(|m| !(m.as_str()==message&&scenario!="noise"&&scenario!="blocker")&& !(m.as_str()=="CQ K1ABC FN42"&&(scenario=="mix"||scenario=="blocker"))).count();
            println!("{scenario}\t{seed}\t{snr}\tnative\t{correct}\t{wrong}\t{:.3}\t{}",t.elapsed().as_secs_f64()*1000.0,rows.join(";"));
            if blind {
                let methods=if sic {vec!["multi","phase","sic-native","sic-constant"]} else {vec!["single","multi","phase"]};
                for method in methods {
                    let t=Instant::now();let rows=match method {
                        "sic-native"=>search_cancel(&audio,"native"),"sic-constant"=>search_cancel(&audio,"constant"),
                        _=>search(&audio,method!="single",method=="phase"),
                    };
                    let correct=usize::from(rows.iter().any(|r|r.msg==message)&&scenario!="noise"&&scenario!="blocker");
                    let wrong=rows.iter().filter(|r|!(r.msg==message&&scenario!="noise"&&scenario!="blocker")&&!(r.msg=="CQ K1ABC FN42"&&(scenario=="mix"||scenario=="blocker"))).count();
                    let details=rows.iter().map(|r|format!("{}@{:.3}/{:.3}/{}",r.msg,r.f,r.dt,r.method)).collect::<Vec<_>>().join(";");
                    println!("{scenario}\t{seed}\t{snr}\tblind-{method}\t{correct}\t{wrong}\t{:.3}\t{details}",t.elapsed().as_secs_f64()*1000.0);
                }
                continue;
            }
            let band=Band::new(&audio,1000.0);let s=band.spectra(f,dt,drift);
            for block in 1..=4 {for mode in ["amp","power","lse","bessel","robust"] {
                let t=Instant::now();let row=decode(&metrics(&s,block,mode));
                let correct=usize::from(row.as_deref()==Some(message)&&scenario!="noise"&&scenario!="blocker");
                let wrong=usize::from(row.is_some()&&correct==0);
                println!("{scenario}\t{seed}\t{snr}\toracle-{block}-{mode}\t{correct}\t{wrong}\t{:.3}\t{}",t.elapsed().as_secs_f64()*1000.0,row.unwrap_or_default());
            }}
            for scheme in 0..3 {
                let t=Instant::now();let row=decode(&coherent_metrics(&s,scheme));
                let correct=usize::from(row.as_deref()==Some(message)&&scenario!="noise"&&scenario!="blocker");
                let wrong=usize::from(row.is_some()&&correct==0);
                println!("{scenario}\t{seed}\t{snr}\toracle-coherent-{scheme}\t{correct}\t{wrong}\t{:.3}\t{}",t.elapsed().as_secs_f64()*1000.0,row.unwrap_or_default());
            }
        }
    }
}
