mod power_request;

fn main() -> std::process::ExitCode {
    power_request::run_cli(power_request::PowerAction::PowerOff)
}
