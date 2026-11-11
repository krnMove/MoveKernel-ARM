use crate::{fat32::Fat32, println, scheduler, virtio_keyboard};

const COMMAND_CAPACITY: usize = 256;

pub fn run(volume: Option<Fat32>) -> ! {
    let mut command = [0u8; COMMAND_CAPACITY];
    let mut length = 0;
    prompt();

    loop {
        scheduler::run_one();
        match virtio_keyboard::poll_key()
            .unwrap_or_else(|error| panic!("keyboard polling failed: {error}"))
        {
            Some(virtio_keyboard::Key::Character(character)) if character.is_ascii() => {
                if length < command.len() {
                    command[length] = character as u8;
                    length += 1;
                    crate::print!("{}", character)
                        .unwrap_or_else(|_| panic!("virtio-gpu output failed"));
                }
            }
            Some(virtio_keyboard::Key::Backspace) if length > 0 => {
                length -= 1;
                crate::console::backspace().unwrap_or_else(|_| panic!("virtio-gpu output failed"));
            }
            Some(virtio_keyboard::Key::Enter) => {
                println!().unwrap_or_else(|_| panic!("virtio-gpu output failed"));
                let verb_length = command[..length]
                    .iter()
                    .position(u8::is_ascii_whitespace)
                    .unwrap_or(length);
                command[..verb_length].make_ascii_lowercase();
                execute(&command[..length], volume);
                length = 0;
                prompt();
            }
            _ => core::hint::spin_loop(),
        }
    }
}

fn prompt() {
    crate::print!("MoveKernel> ").unwrap_or_else(|_| panic!("virtio-gpu output failed"));
}

fn execute(command: &[u8], volume: Option<Fat32>) {
    let command = core::str::from_utf8(command).expect("shell input must be ASCII");
    let mut parts = command.split_ascii_whitespace();
    let verb = parts.next().unwrap_or("");
    let verb_length = verb.len();
    match verb {
        "" => {}
        "/help" => {
            println!("COMMANDS:").unwrap_or_else(|_| panic!("virtio-gpu output failed"));
            println!("/HELP - show this list")
                .unwrap_or_else(|_| panic!("virtio-gpu output failed"));
            println!("/LS - list files in the root directory")
                .unwrap_or_else(|_| panic!("virtio-gpu output failed"));
            println!("/CAT <NAME.EXT> - read a file")
                .unwrap_or_else(|_| panic!("virtio-gpu output failed"));
            println!("/WRITE <NAME.EXT> <TEXT> - write a file")
                .unwrap_or_else(|_| panic!("virtio-gpu output failed"));
            println!("/TASKS - show scheduler tasks")
                .unwrap_or_else(|_| panic!("virtio-gpu output failed"));
            println!("/CLEAR - clear the screen")
                .unwrap_or_else(|_| panic!("virtio-gpu output failed"));
        }
        "/clear" if parts.next().is_none() => {
            crate::console::clear().unwrap_or_else(|_| panic!("virtio-gpu clear failed"));
        }
        "/tasks" if parts.next().is_none() => {
            println!("{} cooperative tasks", scheduler::task_count())
                .unwrap_or_else(|_| panic!("virtio-gpu output failed"));
            for index in 0..scheduler::task_count() {
                println!(
                    "task {}: {} runs",
                    index,
                    scheduler::task_runs(index)
                        .unwrap_or_else(|| panic!("scheduler task counter missing"))
                )
                .unwrap_or_else(|_| panic!("virtio-gpu output failed"));
            }
        }
        "/ls" if parts.next().is_none() => match volume {
            Some(volume) => {
                println!("FILES:").unwrap_or_else(|_| panic!("virtio-gpu output failed"));
                println!("MOVEKERNEL/").unwrap_or_else(|_| panic!("virtio-gpu output failed"));
                if let Err(error) = volume.list_root(|entry| {
                    println!(
                        "- {}{}",
                        entry.name(),
                        if entry.is_directory() { "/" } else { "" }
                    )
                    .unwrap_or_else(|_| panic!("virtio-gpu output failed"));
                    true
                }) {
                    println!("/LS: {}", error)
                        .unwrap_or_else(|_| panic!("virtio-gpu output failed"));
                }
            }
            None => println!("/LS: no FAT32 volume mounted")
                .unwrap_or_else(|_| panic!("virtio-gpu output failed")),
        },
        "/cat" => match (parts.next(), parts.next(), volume) {
            (Some(name), None, Some(volume)) => {
                let mut contents = [0; 1024];
                match volume.read_file_83(name, &mut contents) {
                    Ok(length) => match core::str::from_utf8(&contents[..length]) {
                        Ok(text) => println!("{}", text)
                            .unwrap_or_else(|_| panic!("virtio-gpu output failed")),
                        Err(_) => println!("/CAT: file is not UTF-8 text")
                            .unwrap_or_else(|_| panic!("virtio-gpu output failed")),
                    },
                    Err(error) => println!("/CAT: {}", error)
                        .unwrap_or_else(|_| panic!("virtio-gpu output failed")),
                }
            }
            (Some(_), None, None) => println!("/CAT: no FAT32 volume mounted")
                .unwrap_or_else(|_| panic!("virtio-gpu output failed")),
            _ => println!("usage: /CAT <NAME.EXT>")
                .unwrap_or_else(|_| panic!("virtio-gpu output failed")),
        },
        "/write" => {
            let arguments = command[verb_length..].trim_start();
            let split_at = arguments.find(|character: char| character.is_ascii_whitespace());
            let (name, contents) = match split_at {
                Some(index) => (
                    &arguments[..index],
                    arguments[index..].trim_start().as_bytes(),
                ),
                None => (arguments, &[][..]),
            };
            match (name.is_empty(), volume) {
                (true, _) => println!("usage: /WRITE <NAME.EXT> <TEXT>")
                    .unwrap_or_else(|_| panic!("virtio-gpu output failed")),
                (false, Some(volume)) => match volume.write_file_83(name, contents) {
                    Ok(()) => println!("wrote {} bytes to {}", contents.len(), name)
                        .unwrap_or_else(|_| panic!("virtio-gpu output failed")),
                    Err(error) => println!("/WRITE: {}", error)
                        .unwrap_or_else(|_| panic!("virtio-gpu output failed")),
                },
                (false, None) => println!("/WRITE: no FAT32 volume mounted")
                    .unwrap_or_else(|_| panic!("virtio-gpu output failed")),
            }
        }
        _ => println!("unknown command; type '/HELP'")
            .unwrap_or_else(|_| panic!("virtio-gpu output failed")),
    }
}
