use crate::sema::Type;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum SystemOp {
    EnvExists,
    EnvOr,
    SetEnv,
    RemoveEnv,
    ReadStdin,
    WriteStdout,
    WriteStderr,
    RunProcess,
    RunProcessArgs,
    Panic,
    Exit,
    Assert,
    AssertMessage,
    LoadLibrary,
    LoadSymbol,
    UnloadLibrary,
    PointerIsNull,
    CallCI32One,
}

impl SystemOp {
    pub const ALL: [Self; 18] = [
        Self::EnvExists,
        Self::EnvOr,
        Self::SetEnv,
        Self::RemoveEnv,
        Self::ReadStdin,
        Self::WriteStdout,
        Self::WriteStderr,
        Self::RunProcess,
        Self::RunProcessArgs,
        Self::Panic,
        Self::Exit,
        Self::Assert,
        Self::AssertMessage,
        Self::LoadLibrary,
        Self::LoadSymbol,
        Self::UnloadLibrary,
        Self::PointerIsNull,
        Self::CallCI32One,
    ];

    pub fn symbol(self) -> &'static str {
        match self {
            Self::EnvExists => "ryn_env_exists",
            Self::EnvOr => "ryn_env_or",
            Self::SetEnv => "ryn_env_set",
            Self::RemoveEnv => "ryn_env_remove",
            Self::ReadStdin => "ryn_stdin_read",
            Self::WriteStdout => "ryn_stdout_write",
            Self::WriteStderr => "ryn_stderr_write",
            Self::RunProcess => "ryn_process_run",
            Self::RunProcessArgs => "ryn_process_run_args",
            Self::Panic => "ryn_panic",
            Self::Exit => "ryn_exit",
            Self::Assert => "ryn_assert",
            Self::AssertMessage => "ryn_assert_message",
            Self::LoadLibrary => "ryn_dyn_load_library",
            Self::LoadSymbol => "ryn_dyn_load_symbol",
            Self::UnloadLibrary => "ryn_dyn_unload_library",
            Self::PointerIsNull => "ryn_pointer_is_null",
            Self::CallCI32One => "ryn_call_c_i32_one",
        }
    }

    pub fn parameters(self) -> Vec<Type> {
        use Type::Str;
        let raw_pointer = || Type::RawPointer(crate::sema::intern_pointer_target(Type::U8));
        match self {
            Self::EnvExists | Self::RemoveEnv => vec![Str],
            Self::EnvOr | Self::SetEnv => vec![Str, Str],
            Self::ReadStdin => vec![],
            Self::WriteStdout | Self::WriteStderr | Self::RunProcess => vec![Str],
            Self::RunProcessArgs => vec![
                Str,
                Type::Vec(crate::sema::intern_vec_elem(Type::OwnedString)),
            ],
            Self::Panic => vec![Str],
            Self::Exit => vec![Type::I32],
            Self::Assert => vec![Type::Bool],
            Self::AssertMessage => vec![Type::Bool, Str],
            Self::LoadLibrary => vec![Str],
            Self::LoadSymbol => vec![raw_pointer(), Str],
            Self::UnloadLibrary | Self::PointerIsNull => vec![raw_pointer()],
            Self::CallCI32One => vec![raw_pointer(), Type::I32],
        }
    }

    pub fn result(self) -> Option<Type> {
        match self {
            Self::EnvExists | Self::SetEnv | Self::RemoveEnv => Some(Type::Bool),
            Self::EnvOr | Self::ReadStdin => Some(Type::OwnedString),
            Self::RunProcess | Self::RunProcessArgs => Some(Type::I32),
            Self::WriteStdout
            | Self::WriteStderr
            | Self::Panic
            | Self::Exit
            | Self::Assert
            | Self::AssertMessage => None,
            Self::LoadLibrary | Self::LoadSymbol => Some(Type::RawPointer(
                crate::sema::intern_pointer_target(Type::U8),
            )),
            Self::UnloadLibrary | Self::PointerIsNull => Some(Type::Bool),
            Self::CallCI32One => Some(Type::I32),
        }
    }

    pub fn consumes_argument(self, index: usize) -> bool {
        self == Self::RunProcessArgs && index == 1
    }
}
