//! Embeds the Windows application manifest. Its per-monitor DPI awareness must hold from
//! process start: screens are captured before winit opens a window (which would opt in
//! later), and a DPI-unaware process gets scaled-down coordinates and partial captures.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        embed_manifest::embed_manifest(embed_manifest::new_manifest("ShotCraft.ShotCraft"))?;
    }
    println!("cargo:rerun-if-changed=build.rs");
    Ok(())
}
