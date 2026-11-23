//! The desktop binary. The app lives in the library (`lib.rs`), which is also
//! the `cdylib` the Android build loads.

fn main() -> std::io::Result<()> {
    metal_dash::desktop_main()
}
