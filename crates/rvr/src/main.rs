use std::process::ExitCode;

fn main() -> ExitCode {
    rvr::quiet_on_closed_pipe();
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    ExitCode::from(rvr::run(&args))
}
