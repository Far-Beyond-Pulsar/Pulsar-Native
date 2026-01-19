//! # Code Formatting
//!
//! Formats generated Rust code for readability.
//!
//! The compiler can optionally format generated code using:
//! - prettyplease - For AST-based pretty printing
//! - rustfmt - For production-quality formatting (if available)
//!
//! ## Example
//!
//! Unformatted:
//! ```rust,ignore
//! pub fn main(){if true{print_string("Hi".to_string());}else{}}
//! ```
//!
//! Formatted:
//! ```rust,ignore
//! pub fn main() {
//!     if true {
//!         print_string("Hi".to_string());
//!     } else {
//!     }
//! }
//! ```

/// Format Rust code for readability using prettyplease
///
/// This function attempts to parse the given Rust code and reformat it
/// using the prettyplease library. If parsing fails, the original code
/// is returned unchanged.
///
/// # Arguments
///
/// * `code` - The Rust source code to format
///
/// # Returns
///
/// A formatted version of the code, or the original code if parsing fails
///
/// # Example
///
/// ```rust,ignore
/// let unformatted = "pub fn main(){if true{println!(\"Hi\");}else{}}";
/// let formatted = format_rust_code(unformatted);
/// // Returns nicely formatted code with proper indentation
/// ```
pub fn format_rust_code(code: &str) -> String {
    // Try to parse and re-format using prettyplease
    if let Ok(file) = syn::parse_file(code) {
        prettyplease::unparse(&file)
    } else {
        // If parsing fails, return as-is
        code.to_string()
    }
}
