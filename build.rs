use std::env;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Default)]
struct LwPktOptions {
    max_data_len: Option<usize>,
    use_flags: bool,
}

impl LwPktOptions {
    const END_FILE: &str = "#endif /* LWPKT_OPTS_HDR_H */";

    fn generate(self, path: &Path) {
        let mut origin_f = File::open("lwpkt_opts_template.h").unwrap();

        let mut f = File::create(path).unwrap();

        let mut b = vec![];
        origin_f.read_to_end(&mut b).unwrap();

        f.write_all(&b).unwrap();

        drop(b);
        drop(origin_f);

        if let Some(v) = self.max_data_len {
            f.write_all(format!("\n#define LWPKT_CFG_MAX_DATA_LEN {v}\n").as_bytes())
                .unwrap();
        }

        if self.use_flags {
            f.write_all("\n#define LWPKT_CFG_USE_FLAGS\n".as_bytes())
                .unwrap();
        }

        f.write_all(Self::END_FILE.as_bytes()).unwrap();
        f.flush().unwrap();
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=lwpkt_opts_template.h");
    println!("cargo:rerun-if-env-changed=LWPKT_CFG_MAX_DATA_LEN");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());

    // Tell cargo to look for shared libraries in the specified directory
    // println!("cargo:rustc-link-search=/path/to/lib");

    // Tell cargo to tell rustc to link the system bzip2
    // shared library.
    // println!("cargo:rustc-link-lib=bz2");

    // The bindgen::Builder is the main entry point
    // to bindgen, and lets you build up options for
    // the resulting bindings.

    let mut options = LwPktOptions::default();

    if let Some(v) = std::env::var_os("LWPKT_CFG_MAX_DATA_LEN") {
        options.max_data_len = Some(v.to_str().unwrap().parse::<usize>().unwrap());
    }

    if std::env::var_os("CARGO_FEATURE_FLAGS").is_some() {
        options.use_flags = true;
    }

    if let Ok(branch) = std::env::var("LWPKT_BRANCH") {
        let cmd = std::process::Command::new("git")
            .args(&[
                "--work-tree",
                "./src/lwpkt",
                "--git-dir",
                "./src/lwpkt/.git",
                "checkout",
                "-q",
                &branch,
            ])
            .status()
            .unwrap();
        assert!(cmd.success());
    }

    let out_lwpkt = out_path.join("lwpkt");
    let out_lwrb = out_path.join("lwrb");

    let _ = std::fs::create_dir(&out_lwpkt);
    let _ = std::fs::create_dir(&out_lwrb);

    let options_path = out_lwpkt.join("lwpkt_opts.h");
    options.generate(&options_path);

    let opt_file = out_lwpkt.join("lwpkt_opt.h");
    std::fs::copy("src/lwpkt/lwpkt/src/include/lwpkt/lwpkt_opt.h", &opt_file).unwrap();

    // Second copy for link lwpkt.c
    let lwpk_h = out_lwpkt.join("lwpkt.h");
    std::fs::copy("src/lwpkt/lwpkt/src/include/lwpkt/lwpkt.h", &lwpk_h).unwrap();

    let lwpk_h = out_path.join("lwpkt.h");
    std::fs::copy("src/lwpkt/lwpkt/src/include/lwpkt/lwpkt.h", &lwpk_h).unwrap();

    let lwpk_c = out_path.join("lwpkt.c");
    std::fs::copy("src/lwpkt/lwpkt/src/lwpkt/lwpkt.c", &lwpk_c).unwrap();

    let lwrb_h = out_lwrb.join("lwrb.h");
    std::fs::copy("src/lwpkt/libs/lwrb/src/include/lwrb/lwrb.h", &lwrb_h).unwrap();

    let lwrb_c: PathBuf = out_path.join("lwrb.c");
    std::fs::copy("src/lwpkt/libs/lwrb/src/lwrb/lwrb.c", &lwrb_c).unwrap();

    // let header = &[out_path];
    let c_source = &[lwrb_c, lwpk_c];

    let bindings = bindgen::Builder::default()
        // The input header we would like to generate
        // bindings for.
        .clang_arg("-DLWRB_DISABLE_ATOMIC")
        .header(lwrb_h.into_os_string().into_string().unwrap())
        .header(options_path.into_os_string().into_string().unwrap())
        .header(opt_file.into_os_string().into_string().unwrap())
        .header(lwpk_h.into_os_string().into_string().unwrap())
        // Tell cargo to invalidate the built crate whenever any of the
        // included header files changed.
        .derive_default(true)
        .default_enum_style(bindgen::EnumVariation::ModuleConsts)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        // Finish the builder and generate the bindings.
        .generate()
        // Unwrap the Result and panic on failure.
        .expect("Unable to generate bindings");

    // Write the bindings to the $OUT_DIR/bindings.rs file.

    let mut builder = cc::Build::new();
    builder.files(c_source);
    builder.include(&out_path);
    builder.compile("lwpkt");

    bindings
        .write_to_file(out_path.join("lwpkt.rs"))
        .expect("Couldn't write bindings!");

    println!("cargo:include={}", out_path.to_str().unwrap());
}
