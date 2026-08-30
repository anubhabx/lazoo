use std::{fs::OpenOptions, io::Write as _, path::Path, time::Duration};

fn main() {
    let mut arguments = std::env::args_os().skip(1);
    let mode = arguments.next().expect("fixture mode");
    let pid_file = arguments.next().expect("fixture PID file");
    let mut child = if mode == "--root" {
        append_pid(&pid_file, "root");
        Some(
            std::process::Command::new(std::env::current_exe().expect("fixture executable"))
                .arg("--child")
                .arg(&pid_file)
                .spawn()
                .expect("spawn fixture child"),
        )
    } else if mode == "--child" {
        append_pid(&pid_file, "child");
        None
    } else {
        panic!("unknown fixture mode");
    };
    loop {
        std::thread::sleep(Duration::from_secs(1));
        if let Some(child) = &mut child
            && child.try_wait().expect("poll fixture child").is_some()
        {
            std::process::exit(1);
        }
    }
}

fn append_pid(path: impl AsRef<Path>, label: &str) {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open fixture PID file");
    writeln!(file, "{label}={}", std::process::id()).expect("write fixture PID");
    file.flush().expect("flush fixture PID");
}
