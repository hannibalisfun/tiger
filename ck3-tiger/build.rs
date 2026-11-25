use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let mut revision = None;
    if Path::new("../.git").exists() {
        let mut refs = vec!["HEAD".to_owned(), "packed-refs".to_owned()];
        refs.extend(git(&["symbolic-ref", "-q", "HEAD"]));
        for name in refs {
            if let Some(path) = git(&["rev-parse", "--git-path", &name]) {
                if Path::new(&path).exists() {
                    println!("cargo:rerun-if-changed={path}");
                }
            }
        }
        revision = git(&["rev-parse", "--verify", "HEAD"]);
    }
    println!("cargo:rustc-env=TIGER_REVISION={}", revision.as_deref().unwrap_or("unknown"));
}
