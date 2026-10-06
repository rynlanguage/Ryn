use crate::sema::Type;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum SystemOp {
    EnvExists,
    EnvOr,
    SetEnv,
    RemoveEnv,
    ReadStdin,
    ReadStdinLine,
    Ask,
    WriteStdout,
    FlushStdout,
    WriteStderr,
    FlushStderr,
    RunProcess,
    RunProcessArgs,
    ProcessNew,
    ProcessArg,
    ProcessArgs,
    ProcessEnv,
    ProcessCwd,
    ProcessSpawn,
    ProcessWait,
    ProcessKill,
    ProcessDrop,
    Panic,
    Exit,
    Assert,
    AssertMessage,
    LoadLibrary,
    LoadSymbol,
    UnloadLibrary,
    PointerIsNull,
    CallCI32One,
    MathAbs,
    MathMin,
    MathMax,
    MathClamp,
    MathSqrt,
    MathPow,
    MathFloor,
    MathCeil,
    MathRound,
    MathSin,
    MathCos,
    MathTan,
    MathLog,
    MathLog2,
    MathLog10,
    Random,
    RandomRange,
    Sleep,
    YieldThread,
    EnvGet,
    CurrentDir,
    SetCurrentDir,
    HomeDir,
    TempDir,
    ExecutablePath,
    Os,
    Arch,
    CpuCount,
    Hostname,
    TimeUnix,
    TimeMonotonic,
    TimeElapsed,
    OptionUnwrapFailed,
    OptionExpectFailed,
    ResultUnwrapFailed,
    ProcessCapture,
    ProcessCaptureExitCode,
    ProcessCaptureStdout,
    ProcessCaptureStderr,
    ProcessCaptureDrop,
    TcpConnect,
    TcpRead,
    TcpWrite,
    TcpClose,
    TcpDrop,
    TcpListenerBind,
    TcpAccept,
    TcpListenerClose,
    TcpListenerDrop,
    DnsResolve,
    UdpBind,
    UdpConnect,
    UdpRead,
    UdpWrite,
    UdpClose,
    UdpDrop,
    ThreadSpawn,
    ThreadJoin,
    ThreadId,
    ThreadDrop,
    KeyDown,
    KeyPressed,
    KeyReleased,
    ReadKey,
}

impl SystemOp {
    pub const ALL: [Self; 95] = [
        Self::EnvExists,
        Self::EnvOr,
        Self::SetEnv,
        Self::RemoveEnv,
        Self::ReadStdin,
        Self::ReadStdinLine,
        Self::Ask,
        Self::WriteStdout,
        Self::FlushStdout,
        Self::WriteStderr,
        Self::FlushStderr,
        Self::RunProcess,
        Self::RunProcessArgs,
        Self::ProcessNew,
        Self::ProcessArg,
        Self::ProcessArgs,
        Self::ProcessEnv,
        Self::ProcessCwd,
        Self::ProcessSpawn,
        Self::ProcessWait,
        Self::ProcessKill,
        Self::ProcessDrop,
        Self::Panic,
        Self::Exit,
        Self::Assert,
        Self::AssertMessage,
        Self::LoadLibrary,
        Self::LoadSymbol,
        Self::UnloadLibrary,
        Self::PointerIsNull,
        Self::CallCI32One,
        Self::MathAbs,
        Self::MathMin,
        Self::MathMax,
        Self::MathClamp,
        Self::MathSqrt,
        Self::MathPow,
        Self::MathFloor,
        Self::MathCeil,
        Self::MathRound,
        Self::MathSin,
        Self::MathCos,
        Self::MathTan,
        Self::MathLog,
        Self::MathLog2,
        Self::MathLog10,
        Self::Random,
        Self::RandomRange,
        Self::Sleep,
        Self::YieldThread,
        Self::EnvGet,
        Self::CurrentDir,
        Self::SetCurrentDir,
        Self::HomeDir,
        Self::TempDir,
        Self::ExecutablePath,
        Self::Os,
        Self::Arch,
        Self::CpuCount,
        Self::Hostname,
        Self::TimeUnix,
        Self::TimeMonotonic,
        Self::TimeElapsed,
        Self::OptionUnwrapFailed,
        Self::OptionExpectFailed,
        Self::ResultUnwrapFailed,
        Self::ProcessCapture,
        Self::ProcessCaptureExitCode,
        Self::ProcessCaptureStdout,
        Self::ProcessCaptureStderr,
        Self::ProcessCaptureDrop,
        Self::TcpConnect,
        Self::TcpRead,
        Self::TcpWrite,
        Self::TcpClose,
        Self::TcpDrop,
        Self::TcpListenerBind,
        Self::TcpAccept,
        Self::TcpListenerClose,
        Self::TcpListenerDrop,
        Self::DnsResolve,
        Self::UdpBind,
        Self::UdpConnect,
        Self::UdpRead,
        Self::UdpWrite,
        Self::UdpClose,
        Self::UdpDrop,
        Self::ThreadSpawn,
        Self::ThreadJoin,
        Self::ThreadId,
        Self::ThreadDrop,
        Self::KeyDown,
        Self::KeyPressed,
        Self::KeyReleased,
        Self::ReadKey,
    ];

    pub fn symbol(self) -> &'static str {
        match self {
            Self::EnvExists => "ryn_env_exists",
            Self::EnvOr => "ryn_env_or",
            Self::SetEnv => "ryn_env_set",
            Self::RemoveEnv => "ryn_env_remove",
            Self::ReadStdin => "ryn_stdin_read",
            Self::ReadStdinLine => "ryn_stdin_read_line",
            Self::Ask => "ryn_ask",
            Self::WriteStdout => "ryn_stdout_write",
            Self::FlushStdout => "ryn_stdout_flush",
            Self::WriteStderr => "ryn_stderr_write",
            Self::FlushStderr => "ryn_stderr_flush",
            Self::RunProcess => "ryn_process_run",
            Self::RunProcessArgs => "ryn_process_run_args",
            Self::ProcessNew => "ryn_process_new",
            Self::ProcessArg => "ryn_process_arg",
            Self::ProcessArgs => "ryn_process_args",
            Self::ProcessEnv => "ryn_process_env",
            Self::ProcessCwd => "ryn_process_cwd",
            Self::ProcessSpawn => "ryn_process_spawn",
            Self::ProcessWait => "ryn_process_wait",
            Self::ProcessKill => "ryn_process_kill",
            Self::ProcessDrop => "ryn_process_drop",
            Self::Panic => "ryn_panic",
            Self::Exit => "ryn_exit",
            Self::Assert => "ryn_assert",
            Self::AssertMessage => "ryn_assert_message",
            Self::LoadLibrary => "ryn_dyn_load_library",
            Self::LoadSymbol => "ryn_dyn_load_symbol",
            Self::UnloadLibrary => "ryn_dyn_unload_library",
            Self::PointerIsNull => "ryn_pointer_is_null",
            Self::CallCI32One => "ryn_call_c_i32_one",
            Self::MathAbs => "ryn_math_abs",
            Self::MathMin => "ryn_math_min",
            Self::MathMax => "ryn_math_max",
            Self::MathClamp => "ryn_math_clamp",
            Self::MathSqrt => "ryn_math_sqrt",
            Self::MathPow => "ryn_math_pow",
            Self::MathFloor => "ryn_math_floor",
            Self::MathCeil => "ryn_math_ceil",
            Self::MathRound => "ryn_math_round",
            Self::MathSin => "ryn_math_sin",
            Self::MathCos => "ryn_math_cos",
            Self::MathTan => "ryn_math_tan",
            Self::MathLog => "ryn_math_log",
            Self::MathLog2 => "ryn_math_log2",
            Self::MathLog10 => "ryn_math_log10",
            Self::Random => "ryn_random",
            Self::RandomRange => "ryn_random_range",
            Self::Sleep => "ryn_sleep",
            Self::YieldThread => "ryn_yield_thread",
            Self::EnvGet => "ryn_env_get",
            Self::CurrentDir => "ryn_current_dir",
            Self::SetCurrentDir => "ryn_set_current_dir",
            Self::HomeDir => "ryn_home_dir",
            Self::TempDir => "ryn_temp_dir",
            Self::ExecutablePath => "ryn_executable_path",
            Self::Os => "ryn_os",
            Self::Arch => "ryn_arch",
            Self::CpuCount => "ryn_cpu_count",
            Self::Hostname => "ryn_hostname",
            Self::TimeUnix => "ryn_time_unix",
            Self::TimeMonotonic => "ryn_time_monotonic",
            Self::TimeElapsed => "ryn_time_elapsed",
            Self::OptionUnwrapFailed => "ryn_option_unwrap_failed",
            Self::OptionExpectFailed => "ryn_option_expect_failed",
            Self::ResultUnwrapFailed => "ryn_result_unwrap_failed",
            Self::ProcessCapture => "ryn_process_capture",
            Self::ProcessCaptureExitCode => "ryn_process_capture_exit_code",
            Self::ProcessCaptureStdout => "ryn_process_capture_stdout",
            Self::ProcessCaptureStderr => "ryn_process_capture_stderr",
            Self::ProcessCaptureDrop => "ryn_process_capture_drop",
            Self::TcpConnect => "ryn_tcp_connect",
            Self::TcpRead => "ryn_tcp_read",
            Self::TcpWrite => "ryn_tcp_write",
            Self::TcpClose => "ryn_tcp_close",
            Self::TcpDrop => "ryn_tcp_drop",
            Self::TcpListenerBind => "ryn_tcp_listener_bind",
            Self::TcpAccept => "ryn_tcp_accept",
            Self::TcpListenerClose => "ryn_tcp_listener_close",
            Self::TcpListenerDrop => "ryn_tcp_listener_drop",
            Self::DnsResolve => "ryn_dns_resolve",
            Self::UdpBind => "ryn_udp_bind",
            Self::UdpConnect => "ryn_udp_connect",
            Self::UdpRead => "ryn_udp_read",
            Self::UdpWrite => "ryn_udp_write",
            Self::UdpClose => "ryn_udp_close",
            Self::UdpDrop => "ryn_udp_drop",
            Self::ThreadSpawn => "ryn_thread_spawn",
            Self::ThreadJoin => "ryn_thread_join",
            Self::ThreadId => "ryn_thread_id",
            Self::ThreadDrop => "ryn_thread_drop",
            Self::KeyDown => "ryn_input_key_down",
            Self::KeyPressed => "ryn_input_key_pressed",
            Self::KeyReleased => "ryn_input_key_released",
            Self::ReadKey => "ryn_input_read_key",
        }
    }

    pub fn parameters(self) -> Vec<Type> {
        use Type::Str;
        let raw_pointer = || Type::RawPointer(crate::sema::intern_pointer_target(Type::U8));
        match self {
            Self::EnvExists | Self::RemoveEnv => vec![Str],
            Self::EnvOr | Self::SetEnv => vec![Str, Str],
            Self::ReadStdin | Self::ReadStdinLine | Self::FlushStdout | Self::FlushStderr => vec![],
            Self::Ask => vec![Str],
            Self::WriteStdout | Self::WriteStderr | Self::RunProcess => vec![Str],
            Self::RunProcessArgs => vec![
                Str,
                Type::Vec(crate::sema::intern_vec_elem(Type::OwnedString)),
            ],
            Self::ProcessNew => vec![Str],
            Self::ProcessArg | Self::ProcessCwd => vec![raw_pointer(), Str],
            Self::ProcessArgs => vec![
                raw_pointer(),
                Type::Vec(crate::sema::intern_vec_elem(Type::OwnedString)),
            ],
            Self::ProcessEnv => vec![raw_pointer(), Str, Str],
            Self::ProcessSpawn | Self::ProcessWait | Self::ProcessKill | Self::ProcessDrop => {
                vec![raw_pointer()]
            }
            Self::Panic => vec![Str],
            Self::Exit => vec![Type::I32],
            Self::Assert => vec![Type::Bool],
            Self::AssertMessage => vec![Type::Bool, Str],
            Self::LoadLibrary => vec![Str],
            Self::LoadSymbol => vec![raw_pointer(), Str],
            Self::UnloadLibrary | Self::PointerIsNull => vec![raw_pointer()],
            Self::CallCI32One => vec![raw_pointer(), Type::I32],
            Self::MathAbs
            | Self::MathSqrt
            | Self::MathFloor
            | Self::MathCeil
            | Self::MathRound
            | Self::MathSin
            | Self::MathCos
            | Self::MathTan
            | Self::MathLog
            | Self::MathLog2
            | Self::MathLog10 => vec![Type::F64],
            Self::MathMin | Self::MathMax | Self::MathPow | Self::RandomRange => {
                vec![Type::F64, Type::F64]
            }
            Self::MathClamp => vec![Type::F64, Type::F64, Type::F64],
            Self::TimeElapsed => vec![Type::F64],
            Self::Random => vec![],
            Self::Sleep => vec![Type::U64],
            Self::YieldThread => vec![],
            Self::EnvGet | Self::SetCurrentDir => vec![Str],
            Self::CurrentDir
            | Self::HomeDir
            | Self::TempDir
            | Self::ExecutablePath
            | Self::Os
            | Self::Arch
            | Self::CpuCount
            | Self::Hostname => vec![],
            Self::TimeUnix | Self::TimeMonotonic => vec![],
            Self::OptionUnwrapFailed => vec![],
            Self::OptionExpectFailed => vec![Str],
            Self::ResultUnwrapFailed => vec![],
            Self::ProcessCapture => vec![
                Str,
                Type::Vec(crate::sema::intern_vec_elem(Type::OwnedString)),
            ],
            Self::ProcessCaptureExitCode
            | Self::ProcessCaptureStdout
            | Self::ProcessCaptureStderr
            | Self::ProcessCaptureDrop => vec![raw_pointer()],
            Self::TcpConnect | Self::TcpListenerBind | Self::DnsResolve => vec![Str],
            Self::TcpRead
            | Self::TcpClose
            | Self::TcpDrop
            | Self::TcpAccept
            | Self::TcpListenerClose
            | Self::TcpListenerDrop => vec![raw_pointer()],
            Self::TcpWrite => vec![raw_pointer(), Str],
            Self::UdpBind => vec![Str],
            Self::UdpConnect => vec![raw_pointer(), Str],
            Self::UdpRead | Self::UdpClose | Self::UdpDrop => vec![raw_pointer()],
            Self::UdpWrite => vec![raw_pointer(), Str],
            Self::ThreadSpawn => vec![Type::FunctionPointer(crate::sema::intern_function_pointer(
                crate::sema::FunctionPointerSignature {
                    parameters: vec![],
                    result: None,
                    extern_c: true,
                },
            ))],
            Self::ThreadJoin | Self::ThreadId | Self::ThreadDrop => vec![raw_pointer()],
            Self::KeyDown | Self::KeyPressed | Self::KeyReleased => vec![Type::U32],
            Self::ReadKey => vec![],
        }
    }

    pub fn result(self) -> Option<Type> {
        match self {
            Self::EnvExists | Self::SetEnv | Self::RemoveEnv => Some(Type::Bool),
            Self::EnvOr | Self::ReadStdin | Self::ReadStdinLine | Self::Ask => {
                Some(Type::OwnedString)
            }
            Self::RunProcess | Self::RunProcessArgs => Some(Type::I32),
            Self::ProcessNew => Some(Type::RawPointer(crate::sema::intern_pointer_target(
                Type::U8,
            ))),
            Self::ProcessSpawn | Self::ProcessKill => Some(Type::Bool),
            Self::ProcessWait => Some(Type::I32),
            Self::ProcessArg | Self::ProcessArgs | Self::ProcessEnv | Self::ProcessCwd => {
                Some(Type::Bool)
            }
            Self::WriteStdout
            | Self::WriteStderr
            | Self::FlushStdout
            | Self::FlushStderr
            | Self::Panic
            | Self::Exit
            | Self::Assert
            | Self::AssertMessage => None,
            Self::LoadLibrary | Self::LoadSymbol => Some(Type::RawPointer(
                crate::sema::intern_pointer_target(Type::U8),
            )),
            Self::UnloadLibrary | Self::PointerIsNull => Some(Type::Bool),
            Self::CallCI32One => Some(Type::I32),
            Self::MathAbs
            | Self::MathMin
            | Self::MathMax
            | Self::MathClamp
            | Self::MathSqrt
            | Self::MathPow
            | Self::MathFloor
            | Self::MathCeil
            | Self::MathRound
            | Self::MathSin
            | Self::MathCos
            | Self::MathTan
            | Self::MathLog
            | Self::MathLog2
            | Self::MathLog10
            | Self::Random
            | Self::RandomRange => Some(Type::F64),
            Self::Sleep | Self::YieldThread => None,
            Self::EnvGet
            | Self::CurrentDir
            | Self::HomeDir
            | Self::TempDir
            | Self::ExecutablePath
            | Self::Os
            | Self::Arch
            | Self::Hostname => Some(Type::OwnedString),
            Self::SetCurrentDir => Some(Type::Bool),
            Self::CpuCount => Some(Type::U32),
            Self::TimeUnix | Self::TimeMonotonic | Self::TimeElapsed => Some(Type::F64),
            Self::OptionUnwrapFailed => None,
            Self::OptionExpectFailed => None,
            Self::ResultUnwrapFailed => None,
            Self::ProcessCapture => Some(Type::RawPointer(crate::sema::intern_pointer_target(
                Type::U8,
            ))),
            Self::ProcessCaptureExitCode => Some(Type::I32),
            Self::ProcessCaptureStdout | Self::ProcessCaptureStderr => Some(Type::OwnedString),
            Self::ProcessCaptureDrop => None,
            Self::TcpConnect | Self::TcpListenerBind | Self::TcpAccept => Some(Type::RawPointer(
                crate::sema::intern_pointer_target(Type::U8),
            )),
            Self::TcpRead => Some(Type::OwnedString),
            Self::TcpWrite | Self::TcpClose | Self::TcpListenerClose => Some(Type::Bool),
            Self::TcpDrop | Self::TcpListenerDrop => None,
            Self::DnsResolve => Some(Type::Vec(crate::sema::intern_vec_elem(Type::OwnedString))),
            Self::UdpBind => Some(Type::RawPointer(crate::sema::intern_pointer_target(
                Type::U8,
            ))),
            Self::UdpConnect => Some(Type::Bool),
            Self::UdpRead => Some(Type::OwnedString),
            Self::UdpWrite | Self::UdpClose => Some(Type::Bool),
            Self::UdpDrop => None,
            Self::ThreadSpawn => Some(Type::RawPointer(crate::sema::intern_pointer_target(
                Type::U8,
            ))),
            Self::ThreadJoin => Some(Type::Bool),
            Self::ThreadId => Some(Type::U64),
            Self::ThreadDrop => None,
            Self::KeyDown | Self::KeyPressed | Self::KeyReleased => Some(Type::Bool),
            Self::ReadKey => Some(Type::U32),
            Self::ProcessDrop => None,
        }
    }

    pub fn consumes_argument(self, index: usize) -> bool {
        self == Self::RunProcessArgs && index == 1
    }
}
