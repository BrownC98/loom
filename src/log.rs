pub fn now() -> String {
    jiff::Zoned::now()
        .strftime("%Y-%m-%d %H:%M:%S%.3f")
        .to_string()
}

#[allow(clippy::disallowed_macros)]
pub fn write(line: std::fmt::Arguments) {
    eprintln!("{} {line}", now());
}

macro_rules! logln {
    ($($arg:tt)*) => {
        $crate::log::write(format_args!($($arg)*))
    };
}
pub(crate) use logln;
