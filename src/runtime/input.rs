//! Non-blocking keyboard state polling. Key IDs are stable across platform backends.
use std::sync::{Mutex, OnceLock};

#[cfg(not(any(windows, target_os = "linux")))]
use std::io::Write;
#[cfg(any(windows, target_os = "linux"))]
use std::time::Duration;

const KEY_COUNT: usize = 108;
const KEY_IDS: &[u32] = &[
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
    26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49,
    50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73,
    74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97,
    98, 99, 100, 101, 102, 103, 104, 105, 106, 107,
];

struct InputState {
    down: [bool; KEY_COUNT],
    pressed: [bool; KEY_COUNT],
    released: [bool; KEY_COUNT],
}

impl Default for InputState {
    fn default() -> Self {
        Self {
            down: [false; KEY_COUNT],
            pressed: [false; KEY_COUNT],
            released: [false; KEY_COUNT],
        }
    }
}

static STATE: OnceLock<Mutex<InputState>> = OnceLock::new();

fn state() -> &'static Mutex<InputState> {
    STATE.get_or_init(|| Mutex::new(InputState::default()))
}

fn apply_samples(state: &mut InputState, samples: &[(u32, bool)]) {
    for &(key, is_down) in samples {
        let index = key as usize;
        if index == 0 || index >= KEY_COUNT {
            continue;
        }
        if is_down && !state.down[index] {
            state.pressed[index] = true;
        } else if !is_down && state.down[index] {
            state.released[index] = true;
        }
        state.down[index] = is_down;
    }
}

#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    fn GetAsyncKeyState(virtual_key: i32) -> i16;
}

#[cfg(windows)]
fn virtual_keys(key: u32) -> &'static [u16] {
    match key {
        1 => &[0x41],   // A
        2 => &[0x42],   // B
        3 => &[0x43],   // C
        4 => &[0x44],   // D
        5 => &[0x45],   // E
        6 => &[0x46],   // F
        7 => &[0x47],   // G
        8 => &[0x48],   // H
        9 => &[0x49],   // I
        10 => &[0x4a],  // J
        11 => &[0x4b],  // K
        12 => &[0x4c],  // L
        13 => &[0x4d],  // M
        14 => &[0x4e],  // N
        15 => &[0x4f],  // O
        16 => &[0x50],  // P
        17 => &[0x51],  // Q
        18 => &[0x52],  // R
        19 => &[0x53],  // S
        20 => &[0x54],  // T
        21 => &[0x55],  // U
        22 => &[0x56],  // V
        23 => &[0x57],  // W
        24 => &[0x58],  // X
        25 => &[0x59],  // Y
        26 => &[0x5a],  // Z
        27 => &[0x30],  // Digit0
        28 => &[0x31],  // Digit1
        29 => &[0x32],  // Digit2
        30 => &[0x33],  // Digit3
        31 => &[0x34],  // Digit4
        32 => &[0x35],  // Digit5
        33 => &[0x36],  // Digit6
        34 => &[0x37],  // Digit7
        35 => &[0x38],  // Digit8
        36 => &[0x39],  // Digit9
        37 => &[0x20],  // Space
        38 => &[0xd],   // Enter
        39 => &[0x1b],  // Escape
        40 => &[0x9],   // Tab
        41 => &[0x8],   // Backspace
        42 => &[0x2e],  // Delete
        43 => &[0x2d],  // Insert
        44 => &[0x24],  // Home
        45 => &[0x23],  // End
        46 => &[0x21],  // PageUp
        47 => &[0x22],  // PageDown
        48 => &[0x26],  // ArrowUp
        49 => &[0x28],  // ArrowDown
        50 => &[0x25],  // ArrowLeft
        51 => &[0x27],  // ArrowRight
        52 => &[0x10],  // Shift
        53 => &[0xa0],  // LeftShift
        54 => &[0xa1],  // RightShift
        55 => &[0x11],  // Ctrl
        56 => &[0xa2],  // LeftCtrl
        57 => &[0xa3],  // RightCtrl
        58 => &[0x12],  // Alt
        59 => &[0xa4],  // LeftAlt
        60 => &[0xa5],  // RightAlt
        61 => &[0x14],  // CapsLock
        62 => &[0x90],  // NumLock
        63 => &[0x91],  // ScrollLock
        64 => &[0x70],  // F1
        65 => &[0x71],  // F2
        66 => &[0x72],  // F3
        67 => &[0x73],  // F4
        68 => &[0x74],  // F5
        69 => &[0x75],  // F6
        70 => &[0x76],  // F7
        71 => &[0x77],  // F8
        72 => &[0x78],  // F9
        73 => &[0x79],  // F10
        74 => &[0x7a],  // F11
        75 => &[0x7b],  // F12
        76 => &[0xc0],  // Backtick
        77 => &[0xbd],  // Minus
        78 => &[0xbb],  // Equal
        79 => &[0xdb],  // LeftBracket
        80 => &[0xdd],  // RightBracket
        81 => &[0xdc],  // Backslash
        82 => &[0xba],  // Semicolon
        83 => &[0xde],  // Quote
        84 => &[0xbc],  // Comma
        85 => &[0xbe],  // Period
        86 => &[0xbf],  // Slash
        87 => &[0x60],  // Numpad0
        88 => &[0x61],  // Numpad1
        89 => &[0x62],  // Numpad2
        90 => &[0x63],  // Numpad3
        91 => &[0x64],  // Numpad4
        92 => &[0x65],  // Numpad5
        93 => &[0x66],  // Numpad6
        94 => &[0x67],  // Numpad7
        95 => &[0x68],  // Numpad8
        96 => &[0x69],  // Numpad9
        97 => &[0x6b],  // NumpadAdd
        98 => &[0x6d],  // NumpadSubtract
        99 => &[0x6a],  // NumpadMultiply
        100 => &[0x6f], // NumpadDivide
        101 => &[0x6e], // NumpadDecimal
        102 => &[0xd],  // NumpadEnter
        103 => &[0x2c], // PrintScreen
        104 => &[0x13], // Pause
        105 => &[0x5d], // Menu
        106 => &[0x5b], // LeftSuper
        107 => &[0x5c], // RightSuper
        _ => &[],
    }
}

#[cfg(windows)]
fn is_down(key: u32) -> bool {
    virtual_keys(key).iter().any(|&virtual_key| {
        // SAFETY: GetAsyncKeyState is a side-effect-free query for a valid virtual key.
        unsafe { GetAsyncKeyState(i32::from(virtual_key)) < 0 }
    })
}

fn refresh_key(key: u32) {
    if key as usize >= KEY_COUNT {
        return;
    }
    #[cfg(windows)]
    {
        let mut current = state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        apply_samples(&mut current, &[(key, is_down(key))]);
    }
    #[cfg(target_os = "linux")]
    refresh_all();
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = key;
}

fn refresh_all() {
    #[cfg(windows)]
    {
        let mut current = state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for &key in KEY_IDS {
            apply_samples(&mut current, &[(key, is_down(key))]);
        }
    }
    #[cfg(target_os = "linux")]
    {
        let samples = linux::poll_samples();
        let mut current = state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        apply_samples(&mut current, &samples);
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{KEY_COUNT, KEY_IDS};
    use std::{
        fs::{self, File, OpenOptions},
        mem::MaybeUninit,
        os::unix::fs::OpenOptionsExt,
        sync::{Mutex, OnceLock},
        time::{Duration, Instant},
    };

    const EV_KEY: u16 = 0x01;

    #[repr(C)]
    struct InputEvent {
        time: libc::timeval,
        kind: u16,
        code: u16,
        value: i32,
    }

    struct Backend {
        devices: Vec<File>,
        terminal_deadline: [Option<Instant>; KEY_COUNT],
    }

    impl Backend {
        fn new() -> Self {
            let mut devices = Vec::new();
            if let Ok(entries) = fs::read_dir("/dev/input") {
                for entry in entries
                    .flatten()
                    .filter(|entry| entry.file_name().to_string_lossy().starts_with("event"))
                {
                    if let Ok(device) = OpenOptions::new()
                        .read(true)
                        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
                        .open(entry.path())
                    {
                        devices.push(device);
                    }
                }
            }
            Self {
                devices,
                terminal_deadline: [None; KEY_COUNT],
            }
        }

        fn drain_evdev(&mut self, samples: &mut Vec<(u32, bool)>) {
            for device in &mut self.devices {
                loop {
                    let mut event = MaybeUninit::<InputEvent>::uninit();
                    // SAFETY: read writes at most one complete input_event into the aligned,
                    // writable MaybeUninit buffer. The value is read only for a full struct.
                    let read = unsafe {
                        libc::read(
                            std::os::fd::AsRawFd::as_raw_fd(device),
                            event.as_mut_ptr().cast(),
                            std::mem::size_of::<InputEvent>(),
                        )
                    };
                    if read != std::mem::size_of::<InputEvent>() as isize {
                        break;
                    }
                    // SAFETY: the exact InputEvent byte count was returned above.
                    let event = unsafe { event.assume_init() };
                    if event.kind == EV_KEY && (event.value == 0 || event.value == 1) {
                        samples.extend(
                            linux_key_ids(event.code)
                                .iter()
                                .map(|&key| (key, event.value == 1)),
                        );
                    }
                }
            }
        }

        fn drain_terminal(&mut self, samples: &mut Vec<(u32, bool)>) {
            let keys = read_terminal_keys();
            let now = Instant::now();
            for key in keys {
                samples.push((key, true));
                self.terminal_deadline[key as usize] = Some(now + Duration::from_millis(100));
            }
            for key in KEY_IDS.iter().copied().skip(1) {
                if self.terminal_deadline[key as usize].is_some_and(|deadline| deadline <= now) {
                    samples.push((key, false));
                    self.terminal_deadline[key as usize] = None;
                }
            }
        }
    }

    static BACKEND: OnceLock<Mutex<Backend>> = OnceLock::new();

    fn backend() -> &'static Mutex<Backend> {
        BACKEND.get_or_init(|| Mutex::new(Backend::new()))
    }

    pub(super) fn poll_samples() -> Vec<(u32, bool)> {
        let mut samples = Vec::new();
        let mut backend = backend()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        backend.drain_evdev(&mut samples);
        backend.drain_terminal(&mut samples);
        samples
    }

    fn read_terminal_keys() -> Vec<u32> {
        let fd = libc::STDIN_FILENO;
        let mut original = MaybeUninit::<libc::termios>::uninit();
        // SAFETY: tcgetattr initializes the termios structure for this valid descriptor.
        if unsafe { libc::tcgetattr(fd, original.as_mut_ptr()) } != 0 {
            return Vec::new();
        }
        // SAFETY: tcgetattr succeeded.
        let original = unsafe { original.assume_init() };
        let mut raw = original;
        // SAFETY: cfmakeraw only mutates the provided termios value.
        unsafe { libc::cfmakeraw(&mut raw) };
        raw.c_cc[libc::VMIN] = 0;
        raw.c_cc[libc::VTIME] = 0;
        // Keep raw mode active only for the nonblocking read. Line-based stdin remains
        // canonical between polls, and terminal settings are restored before returning.
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
            return Vec::new();
        }
        let mut bytes = [0u8; 64];
        // SAFETY: bytes is a writable 64-byte buffer; VMIN/VTIME are zero, so this does not wait.
        let count = unsafe { libc::read(fd, bytes.as_mut_ptr().cast(), bytes.len()) };
        // SAFETY: original is the exact termios state saved above.
        let restored = unsafe { libc::tcsetattr(fd, libc::TCSANOW, &original) } == 0;
        if !restored || count <= 0 {
            return Vec::new();
        }
        decode_terminal_bytes(&bytes[..count as usize])
    }

    fn decode_terminal_bytes(bytes: &[u8]) -> Vec<u32> {
        let mut keys = Vec::new();
        let mut index = 0;
        while index < bytes.len() {
            let byte = bytes[index];
            if byte == 0x1b {
                if bytes.get(index + 1) == Some(&b'[') {
                    let sequence_start = index + 2;
                    let mut end = sequence_start;
                    while end < bytes.len()
                        && !bytes[end].is_ascii_alphabetic()
                        && bytes[end] != b'~'
                    {
                        end += 1;
                    }
                    if let Some(&last) = bytes.get(end) {
                        let key = match last {
                            b'A' => Some(48),
                            b'B' => Some(49),
                            b'C' => Some(51),
                            b'D' => Some(50),
                            b'H' => Some(44),
                            b'F' => Some(45),
                            b'~' => match &bytes[sequence_start..end] {
                                b"2" => Some(43),
                                b"3" => Some(42),
                                b"5" => Some(46),
                                b"6" => Some(47),
                                b"11" => Some(64),
                                b"12" => Some(65),
                                b"13" => Some(66),
                                b"14" => Some(67),
                                b"15" => Some(68),
                                b"17" => Some(69),
                                b"18" => Some(70),
                                b"19" => Some(71),
                                b"20" => Some(72),
                                b"21" => Some(73),
                                b"23" => Some(74),
                                b"24" => Some(75),
                                _ => None,
                            },
                            _ => None,
                        };
                        if let Some(key) = key {
                            keys.push(key);
                        }
                        index = end + 1;
                        continue;
                    }
                }
                keys.push(39);
                index += 1;
                continue;
            }
            if let Some(key) = ascii_key_id(byte) {
                keys.push(key);
                if (1..=26).contains(&byte) {
                    keys.push(55); // Ctrl is reported with control-character input.
                }
            }
            index += 1;
        }
        keys
    }

    fn ascii_key_id(byte: u8) -> Option<u32> {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' => Some(u32::from(byte.to_ascii_uppercase() - b'A' + 1)),
            b'0'..=b'9' => Some(27 + u32::from(byte - b'0')),
            b' ' => Some(37),
            b'\r' | b'\n' => Some(38),
            b'\t' => Some(40),
            0x08 | 0x7f => Some(41),
            b'`' | b'~' => Some(76),
            b'-' | b'_' => Some(77),
            b'=' | b'+' => Some(78),
            b'[' | b'{' => Some(79),
            b']' | b'}' => Some(80),
            b'\\' | b'|' => Some(81),
            b';' | b':' => Some(82),
            b'\'' | b'"' => Some(83),
            b',' | b'<' => Some(84),
            b'.' | b'>' => Some(85),
            b'/' | b'?' => Some(86),
            1..=26 => Some(u32::from(byte)),
            _ => None,
        }
    }

    fn linux_key_ids(code: u16) -> Vec<u32> {
        match code {
            16 => vec![17], // Q
            17 => vec![23], // W
            18 => vec![5],  // E
            19 => vec![18], // R
            20 => vec![20], // T
            21 => vec![25], // Y
            22 => vec![21], // U
            23 => vec![9],  // I
            24 => vec![15], // O
            25 => vec![16], // P
            30 => vec![1],  // A
            31 => vec![19], // S
            32 => vec![4],  // D
            33 => vec![6],  // F
            34 => vec![7],  // G
            35 => vec![8],  // H
            36 => vec![10], // J
            37 => vec![11], // K
            38 => vec![12], // L
            44 => vec![26], // Z
            45 => vec![24], // X
            46 => vec![3],  // C
            47 => vec![22], // V
            48 => vec![2],  // B
            49 => vec![14], // N
            50 => vec![13], // M
            2..=11 => vec![27 + u32::from((code - 2 + 1) % 10)],
            57 => vec![37],
            28 | 96 => vec![38],
            1 => vec![39],
            15 => vec![40],
            14 => vec![41],
            111 => vec![42],
            110 => vec![43],
            102 => vec![44],
            107 => vec![45],
            104 => vec![46],
            109 => vec![47],
            103 => vec![48],
            108 => vec![49],
            105 => vec![50],
            106 => vec![51],
            42 => vec![52, 53],
            54 => vec![52, 54],
            29 => vec![55, 56],
            97 => vec![55, 57],
            56 => vec![58, 59],
            100 => vec![58, 60],
            58 => vec![61],
            69 => vec![62],
            70 => vec![63],
            59..=68 => vec![64 + u32::from(code - 59)],
            87 => vec![74],
            88 => vec![75],
            41 => vec![76],
            12 => vec![77],
            13 => vec![78],
            26 => vec![79],
            27 => vec![80],
            43 => vec![81],
            39 => vec![82],
            40 => vec![83],
            51 => vec![84],
            52 => vec![85],
            53 => vec![86],
            82 => vec![87],
            79 => vec![88],
            80 => vec![89],
            81 => vec![90],
            75 => vec![91],
            76 => vec![92],
            77 => vec![93],
            71 => vec![94],
            72 => vec![95],
            73 => vec![96],
            78 => vec![97],
            74 => vec![98],
            55 => vec![99],
            98 => vec![100],
            83 => vec![101],
            99 => vec![103],
            119 => vec![104],
            127 => vec![105],
            125 => vec![106],
            126 => vec![107],
            _ => Vec::new(),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{ascii_key_id, decode_terminal_bytes, linux_key_ids};

        #[test]
        fn terminal_parser_maps_ascii_and_navigation_sequences() {
            assert_eq!(decode_terminal_bytes(b"w \x1b[A\x1b[3~"), [23, 37, 48, 42]);
            assert_eq!(ascii_key_id(b'Q'), Some(17));
            assert_eq!(ascii_key_id(3), Some(3));
        }

        #[test]
        fn evdev_mapping_matches_the_public_key_ids() {
            assert_eq!(linux_key_ids(17), vec![23]); // W
            assert_eq!(linux_key_ids(57), vec![37]); // Space
            assert_eq!(linux_key_ids(103), vec![48]); // ArrowUp
            assert_eq!(linux_key_ids(42), vec![52, 53]); // LeftShift and Shift
            assert_eq!(linux_key_ids(29), vec![55, 56]); // LeftCtrl and Ctrl
            assert_eq!(linux_key_ids(56), vec![58, 59]); // LeftAlt and Alt
            assert!(linux_key_ids(u16::MAX).is_empty());
        }
    }
}

fn take_edge(key: u32, pressed: bool) -> bool {
    if key as usize >= KEY_COUNT {
        return false;
    }
    refresh_key(key);
    let mut current = state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    take_edge_from_state(&mut current, key, pressed)
}

fn take_edge_from_state(current: &mut InputState, key: u32, pressed: bool) -> bool {
    if key as usize >= KEY_COUNT {
        return false;
    }
    if pressed {
        std::mem::replace(&mut current.pressed[key as usize], false)
    } else {
        std::mem::replace(&mut current.released[key as usize], false)
    }
}

fn take_pressed_key(current: &mut InputState) -> Option<u32> {
    KEY_IDS
        .iter()
        .copied()
        .skip(1)
        .find(|key| std::mem::replace(&mut current.pressed[*key as usize], false))
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_input_key_down(key: u32) -> bool {
    if key as usize >= KEY_COUNT {
        return false;
    }
    refresh_key(key);
    state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .down[key as usize]
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_input_key_pressed(key: u32) -> bool {
    take_edge(key, true)
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_input_key_released(key: u32) -> bool {
    take_edge(key, false)
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_input_read_key() -> u32 {
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = writeln!(
            std::io::stderr().lock(),
            "Ryn runtime error: realtime keyboard input is not supported on this platform"
        );
        std::process::exit(1);
    }
    #[cfg(any(windows, target_os = "linux"))]
    loop {
        refresh_all();
        {
            let mut current = state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(key) = take_pressed_key(&mut current) {
                return key;
            }
        }
        std::thread::sleep(Duration::from_millis(4));
    }
}

#[cfg(test)]
mod tests {
    use super::{InputState, apply_samples, take_edge_from_state, take_pressed_key};

    #[test]
    fn transitions_latch_press_and_release_until_queried() {
        let mut state = InputState::default();
        apply_samples(&mut state, &[(23, true)]);
        assert!(state.down[23]);
        assert!(state.pressed[23]);
        assert!(!state.released[23]);
        apply_samples(&mut state, &[(23, true)]);
        assert!(state.pressed[23]);
        apply_samples(&mut state, &[(23, false)]);
        assert!(!state.down[23]);
        assert!(state.pressed[23]);
        assert!(state.released[23]);
    }

    #[test]
    fn unsupported_key_ids_are_ignored_without_indexing_state() {
        let mut state = InputState::default();
        apply_samples(&mut state, &[(0, true), (u32::MAX, true)]);
        assert!(!state.down[0]);
    }

    #[test]
    fn edge_queries_consume_each_transition_once() {
        let mut state = InputState::default();
        apply_samples(&mut state, &[(23, true)]);
        assert!(take_edge_from_state(&mut state, 23, true));
        assert!(!take_edge_from_state(&mut state, 23, true));
        apply_samples(&mut state, &[(23, false)]);
        assert!(take_edge_from_state(&mut state, 23, false));
        assert!(!take_edge_from_state(&mut state, 23, false));
    }

    #[test]
    fn read_key_takes_the_first_pending_press_and_leaves_other_keys_queued() {
        let mut state = InputState::default();
        apply_samples(&mut state, &[(23, true), (37, true)]);
        assert_eq!(take_pressed_key(&mut state), Some(23));
        assert_eq!(take_pressed_key(&mut state), Some(37));
        assert_eq!(take_pressed_key(&mut state), None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_backend_maps_common_keys_and_modifiers() {
        assert_eq!(super::virtual_keys(23), &[0x57]); // W
        assert_eq!(super::virtual_keys(37), &[0x20]); // Space
        assert_eq!(super::virtual_keys(48), &[0x26]); // ArrowUp
        assert_eq!(super::virtual_keys(52), &[0x10]); // Shift
        assert_eq!(super::virtual_keys(55), &[0x11]); // Ctrl
        assert_eq!(super::virtual_keys(58), &[0x12]); // Alt
    }
}
