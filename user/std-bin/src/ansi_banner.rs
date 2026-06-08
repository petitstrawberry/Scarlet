use std::process::ExitCode;

fn main() -> ExitCode {
    println!();
    println!("\x1b[1;38;2;255;255;255;48;2;39;74;110m Terminal \x1b[0m");
    println!(
        "\x1b[38;5;39mcolor\x1b[0m  \
         \x1b[1mBold\x1b[0m  \
         \x1b[2mFaint\x1b[0m  \
         \x1b[3mItalic attr\x1b[0m  \
         \x1b[4mUnderline\x1b[0m  \
         \x1b[9mStrike\x1b[0m  \
         \x1b[7mInverse\x1b[0m"
    );
    println!();

    for color in 0..8 {
        print!("\x1b[3{color}m normal \x1b[0m");
    }
    println!();
    for color in 0..8 {
        print!("\x1b[9{color}m bright \x1b[0m");
    }
    println!();
    for color in 0..8 {
        print!("\x1b[4{color}m        \x1b[0m");
    }
    println!();
    for color in 0..8 {
        print!("\x1b[10{color}m        \x1b[0m");
    }
    println!();
    println!();

    println!(
        "\x1b[38;5;196m256\x1b[38;5;202m-\x1b[38;5;226mcolor\x1b[38;5;46m palette\x1b[0m  \
         \x1b[38;2;255;128;64mtruecolor foreground\x1b[0m  \
         \x1b[48;2;45;20;80m truecolor background \x1b[0m"
    );
    println!();

    ExitCode::SUCCESS
}
