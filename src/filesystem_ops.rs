use crate::sema::Type;

/// File and directory operations exposed to Ryn programs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum FilesystemOp {
    ReadFile,
    WriteFile,
    CreateDir,
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
}

impl FilesystemOp {
    pub const ALL: [Self; 15] = [
        Self::ReadFile,
        Self::WriteFile,
        Self::CreateDir,
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
    ];

    pub fn symbol(self) -> &'static str {
        match self {
            Self::ReadFile => "ryn_fs_read_file",
            Self::WriteFile => "ryn_fs_write_file",
            Self::CreateDir => "ryn_fs_create_dir",
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
        }
    }

    pub fn parameters(self) -> Vec<Type> {
        match self {
            Self::ReadFile
            | Self::CreateDir
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
            | Self::PathIsAbsolute => {
                vec![Type::Str]
            }
            Self::WriteFile | Self::PathJoin => vec![Type::Str, Type::Str],
        }
    }

    pub fn result(self) -> Type {
        match self {
            Self::ReadFile
            | Self::PathJoin
            | Self::PathParent
            | Self::PathFileName
            | Self::PathExtension => Type::OwnedString,
            Self::ReadDir => Type::Vec(crate::sema::intern_vec_elem(Type::OwnedString)),
            Self::PathIsAbsolute => Type::Bool,
            _ => Type::Bool,
        }
    }
}
