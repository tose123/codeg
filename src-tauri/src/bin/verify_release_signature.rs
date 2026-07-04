use std::path::PathBuf;

fn main() {
    let mut args = std::env::args_os();
    let _program = args.next();
    let asset_path = PathBuf::from(args.next().unwrap_or_default());
    let sig_path = PathBuf::from(args.next().unwrap_or_default());

    if asset_path.as_os_str().is_empty() || sig_path.as_os_str().is_empty() || args.next().is_some()
    {
        eprintln!("usage: verify_release_signature <asset-path> <sig-path>");
        std::process::exit(2);
    }

    let asset = std::fs::read(&asset_path).unwrap_or_else(|err| {
        eprintln!("failed to read asset {}: {err}", asset_path.display());
        std::process::exit(1);
    });
    let sig = std::fs::read_to_string(&sig_path).unwrap_or_else(|err| {
        eprintln!("failed to read signature {}: {err}", sig_path.display());
        std::process::exit(1);
    });

    if let Err(err) = codeg_lib::update::verify::verify_release_signature(&asset, &sig) {
        eprintln!(
            "signature verification failed for {} with {}: {err}",
            asset_path.display(),
            sig_path.display()
        );
        std::process::exit(1);
    }

    println!(
        "signature verified: {} <-> {}",
        asset_path.display(),
        sig_path.display()
    );
}
