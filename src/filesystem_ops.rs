use crate::sema::Type;

/// File and directory operations exposed to Ryn programs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum FilesystemOp {
    ReadFile,
    WriteFile,
    CreateDir,
    CreateDirAll,
    DeleteFile,
    DeleteDir,
    Exists,
    IsFile,
    IsDirectory,
    DeleteDirAll,
    ReadDir,
    PathJoin,
    PathParent,
    PathFileName,
    PathExtension,
    PathIsAbsolute,
    CopyFile,
    Rename,
    PathAbsolute,
    PathCanonical,
    FileOpen,
    FileCreate,
    FileRead,
    FileReadLine,
    FileWrite,
    FileFlush,
    FileClose,
    FileDrop,
    AppendFile,
}

impl FilesystemOp {
    pub const ALL: [Self; 29] = [
        Self::ReadFile,
        Self::WriteFile,
        Self::CreateDir,
        Self::CreateDirAll,
        Self::DeleteFile,
        Self::DeleteDir,
        Self::Exists,
        Self::IsFile,
        Self::IsDirectory,
        Self::DeleteDirAll,
        Self::ReadDir,
        Self::PathJoin,
        Self::PathParent,
        Self::PathFileName,
        Self::PathExtension,
        Self::PathIsAbsolute,
        Self::CopyFile,
        Self::Rename,
        Self::PathAbsolute,
        Self::PathCanonical,
        Self::FileOpen,
        Self::FileCreate,
        Self::FileRead,
        Self::FileReadLine,
        Self::FileWrite,
        Self::FileFlush,
        Self::FileClose,
        Self::FileDrop,
        Self::AppendFile,
    ];

    pub fn symbol(self) -> &'static str {
        match self {
            Self::ReadFile => "ryn_fs_read_file",
            Self::WriteFile => "ryn_fs_write_file",
            Self::CreateDir => "ryn_fs_create_dir",
            Self::CreateDirAll => "ryn_fs_create_dir_all",
            Self::DeleteFile => "ryn_fs_delete_file",
            Self::DeleteDir => "ryn_fs_delete_dir",
            Self::Exists => "ryn_fs_exists",
            Self::IsFile => "ryn_fs_is_file",
            Self::IsDirectory => "ryn_fs_is_directory",
            Self::DeleteDirAll => "ryn_fs_delete_dir_all",
            Self::ReadDir => "ryn_fs_read_dir",
            Self::PathJoin => "ryn_fs_path_join",
            Self::PathParent => "ryn_fs_path_parent",
            Self::PathFileName => "ryn_fs_path_file_name",
            Self::PathExtension => "ryn_fs_path_extension",
            Self::PathIsAbsolute => "ryn_fs_path_is_absolute",
            Self::CopyFile => "ryn_fs_copy_file",
            Self::Rename => "ryn_fs_rename",
            Self::PathAbsolute => "ryn_fs_path_absolute",
            Self::PathCanonical => "ryn_fs_path_canonical",
            Self::FileOpen => "ryn_file_open",
            Self::FileCreate => "ryn_file_create",
            Self::FileRead => "ryn_file_read",
            Self::FileReadLine => "ryn_file_read_line",
            Self::FileWrite => "ryn_file_write",
            Self::FileFlush => "ryn_file_flush",
            Self::FileClose => "ryn_file_close",
            Self::FileDrop => "ryn_file_drop",
            Self::AppendFile => "ryn_fs_append_file",
        }
    }

    pub fn parameters(self) -> Vec<Type> {
        match self {
            Self::ReadFile
            | Self::CreateDir
            | Self::CreateDirAll
            | Self::DeleteFile
            | Self::DeleteDir
            | Self::Exists => vec![Type::Str],
            Self::IsFile
            | Self::IsDirectory
            | Self::DeleteDirAll
            | Self::ReadDir
            | Self::PathParent
            | Self::PathFileName
            | Self::PathExtension
            | Self::PathIsAbsolute
            | Self::PathAbsolute
            | Self::PathCanonical => {
                vec![Type::Str]
            }
            Self::WriteFile | Self::PathJoin | Self::CopyFile | Self::Rename | Self::AppendFile => {
                vec![Type::Str, Type::Str]
            }
            Self::FileOpen | Self::FileCreate => vec![Type::Str],
            Self::FileRead
            | Self::FileReadLine
            | Self::FileFlush
            | Self::FileClose
            | Self::FileDrop => {
                vec![Type::RawPointer(crate::sema::intern_pointer_target(
                    Type::U8,
                ))]
            }
            Self::FileWrite => vec![
                Type::RawPointer(crate::sema::intern_pointer_target(Type::U8)),
                Type::Str,
            ],
        }
    }

    pub fn result(self) -> Type {
        match self {
            Self::ReadFile
            | Self::PathJoin
            | Self::PathParent
            | Self::PathFileName
            | Self::PathExtension => Type::OwnedString,
            Self::PathAbsolute | Self::PathCanonical => Type::OwnedString,
            Self::FileRead | Self::FileReadLine => Type::OwnedString,
            Self::FileOpen | Self::FileCreate => {
                Type::RawPointer(crate::sema::intern_pointer_target(Type::U8))
            }
            Self::ReadDir => Type::Vec(crate::sema::intern_vec_elem(Type::OwnedString)),
            Self::PathIsAbsolute => Type::Bool,
            Self::FileDrop => Type::Bool,
            _ => Type::Bool,
        }
    }
}
