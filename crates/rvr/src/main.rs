use std::process::ExitCode;

fn main() -> ExitCode {
    rvr::quiet_on_closed_pipe();
    let args: Vec<String> = std::env::args().skip(1).collect();
    ExitCode::from(rvr::run(&args))
}
