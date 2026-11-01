use std::os::windows::process::CommandExt;
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn hide_console(command: &mut Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}
