//! Builds the eBPF program of the SRv6 forwarding (feature `srv6`).

fn main() {
    #[cfg(feature = "srv6")]
    build_ebpf();
}

/// Builds `kira-forwarding-ebpf` for the BPF target into `OUT_DIR`.
///
/// Requires a nightly toolchain with the `rust-src` component and `bpf-linker`.
#[cfg(feature = "srv6")]
fn build_ebpf() {
    let root_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../kira-forwarding-ebpf");
    println!("cargo::rerun-if-changed={root_dir}/../kira-forwarding-common");

    let package = aya_build::Package {
        name: "kira-forwarding-ebpf",
        root_dir,
        ..Default::default()
    };
    aya_build::build_ebpf([package], aya_build::Toolchain::default())
        .expect("building the eBPF program of the SRv6 forwarding failed");
}
