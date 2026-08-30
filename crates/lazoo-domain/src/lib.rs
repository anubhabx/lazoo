mod employee;
mod project;

pub use employee::{
    EmployeeValidationError, validate_employee_name, validate_employee_text_fields,
    validate_native_settings,
};
pub use project::{
    GitRepositoryInspection, ProjectError, inspect_git_repository, normalize_windows_identity,
};
