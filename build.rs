fn main() {
    // Link against system libfftw3
    println!("cargo:rustc-link-lib=fftw3");
    
    // Re-run build if FFT sources change
    println!("cargo:rerun-if-changed=src/util/fft.rs");
}
