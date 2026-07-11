pub use engine_fs::UserTypeInfo as TypeInfo;

#[derive(Clone, Debug)]
pub struct NavigateToType {
    pub file_path: Option<std::path::PathBuf>,
    pub type_name: String,
}
