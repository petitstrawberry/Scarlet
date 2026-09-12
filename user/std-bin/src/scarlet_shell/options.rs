use clap::{Parser, ValueEnum};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum ShellMode {
    #[default]
    Desktop,
    Console,
}

#[derive(Debug, Parser)]
#[command(name = "scarlet-shell", about = "Scarlet workspace shell")]
pub struct Options {
    /// Choose the desktop drawer or the full-screen console home.
    #[arg(long, value_enum, default_value_t = ShellMode::Desktop)]
    pub mode: ShellMode,
}
