//! # String Module
//!
//! String manipulation and conversion operations for the Pulsar visual programming system.
//!
//! This module provides pure string manipulation nodes including:
//! - String operations (concatenation, length, trimming)
//! - String searching (contains, starts_with, ends_with)
//! - String manipulation (replace, substring, repeat)
//! - String splitting and joining
//! - Case conversion (uppercase, lowercase)
//! - Type conversions (string to number, number to string, etc.)
//! - String validation and formatting

use crate::{blueprint, bp_doc, NodeTypes};

// =============================================================================
// String Concatenation and Basic Operations
// =============================================================================

/// Concatenates two strings together.
///
/// # Inputs
/// - `a`: The first string
/// - `b`: The second string to append
///
/// # Returns
/// A new string containing the contents of `a` followed by `b`
///
/// # Example
/// If `a` is "Hello, " and `b` is "world!", the output will be "Hello, world!".
///
/// # Notes
/// The output is a simple concatenation. For more complex formatting, use the string_format function.
#[bp_doc("# String Concat")]
#[bp_doc("Joins two strings together.")]
#[blueprint(type: NodeTypes::pure, category: "String", color: "#7ED321")]
pub fn string_concat(a: String, b: String) -> String {
    format!("{}{}", a, b)
}

/// Returns the length of a string.
///
/// # Inputs
/// - `text`: The input string to measure
///
/// # Returns
/// The number of characters in the string (as usize converted to i64)
///
/// # Example
/// If `text` is "hello", the output will be 5.
/// If `text` is "", the output will be 0.
///
/// # Notes
/// The length is the number of Unicode scalar values (not necessarily bytes).
/// Use this function to check if a string is empty, limit input, or display character counts.
#[bp_doc("# String Length")]
#[bp_doc("Returns the number of characters in a string.")]
#[blueprint(type: NodeTypes::pure, category: "String", color: "#7ED321")]
pub fn string_length(text: String) -> usize {
    text.len()
}

/// Checks if a string is empty.
///
/// # Inputs
/// - `text`: The input string to check
///
/// # Returns
/// Returns `true` if the string is empty, `false` otherwise
///
/// # Example
/// If `text` is "", the output will be true.
/// If `text` is "hello", the output will be false.
///
/// # Notes
/// Use this function to validate required fields, check for missing data, or control flow based on string content.
#[bp_doc("# String Is Empty")]
#[bp_doc("Returns true if the string is empty.")]
#[blueprint(type: NodeTypes::pure, category: "String", color: "#7ED321")]
pub fn string_is_empty(text: String) -> bool {
    text.is_empty()
}

// =============================================================================
// String Searching and Pattern Matching
// =============================================================================

/// Checks if a string contains a substring.
///
/// # Inputs
/// - `text`: The string to search within
/// - `substring`: The substring to search for
///
/// # Returns
/// Returns `true` if the substring is found in the input string, `false` otherwise
///
/// # Example
/// If `text` is "hello world" and `substring` is "world", the output will be true.
/// If `substring` is "foo", the output will be false.
///
/// # Notes
/// The search is case-sensitive and matches exact substrings. Use this function for basic string pattern checks.
#[bp_doc("# String Contains")]
#[bp_doc("Returns true if the string contains the specified substring.")]
#[blueprint(type: NodeTypes::pure, category: "String", color: "#7ED321")]
pub fn string_contains(text: String, substring: String) -> bool {
    text.contains(&substring)
}

/// Checks if a string starts with a given prefix.
///
/// # Inputs
/// - `text`: The string to check
/// - `prefix`: The prefix to look for
///
/// # Returns
/// `true` if the string starts with the prefix, `false` otherwise
///
/// # Example
/// If `text` is "hello world" and `prefix` is "hello", the output will be `true`.
/// If `prefix` is "world", the output will be `false`.
///
/// # Notes
/// The check is case-sensitive.
#[bp_doc("# String Starts With")]
#[bp_doc("Returns true if the string starts with the specified prefix.")]
#[blueprint(type: NodeTypes::pure, category: "String", color: "#7ED321")]
pub fn string_starts_with(text: String, prefix: String) -> bool {
    text.starts_with(&prefix)
}

/// Checks if a string ends with a specified suffix.
///
/// # Inputs
/// - `text`: The input string to check
/// - `suffix`: The suffix to check for
///
/// # Returns
/// Returns `true` if the input string ends with the specified suffix, `false` otherwise
///
/// # Example
/// If `text` is "hello.txt" and `suffix` is ".txt", the output will be true.
/// If `text` is "image.png" and `suffix` is ".jpg", the output will be false.
///
/// # Notes
/// The check is case-sensitive. Use this function for file extension checks, string validation, or pattern matching.
#[bp_doc("# String Ends With")]
#[bp_doc("Returns true if the string ends with the specified suffix.")]
#[blueprint(type: NodeTypes::pure, category: "String", color: "#7ED321")]
pub fn string_ends_with(text: String, suffix: String) -> bool {
    text.ends_with(&suffix)
}

// =============================================================================
// String Manipulation
// =============================================================================

/// Replaces all occurrences of a pattern in a string.
///
/// # Inputs
/// - `text`: The original string to perform replacements on
/// - `from`: The substring to search for
/// - `to`: The string to replace each occurrence with
///
/// # Returns
/// A new string with all occurrences of `from` replaced by `to`
///
/// # Example
/// If `text` is "hello world", `from` is "world", and `to` is "there",
/// the output will be "hello there".
///
/// # Notes
/// If the search substring is empty, the original string is returned unchanged.
#[bp_doc("# String Replace")]
#[bp_doc("Replaces all occurrences of a substring with another string.")]
#[blueprint(type: NodeTypes::pure, category: "String", color: "#7ED321")]
pub fn string_replace(text: String, from: String, to: String) -> String {
    text.replace(&from, &to)
}

/// Extracts a substring from a string.
///
/// # Inputs
/// - `text`: The source string
/// - `start`: The start index (inclusive, integer)
/// - `end`: The end index (exclusive, integer)
///
/// # Returns
/// The substring from start to end, or an empty string if indices are invalid
///
/// # Example
/// If `text` is "abcdef", `start` is 1, and `end` is 4, the output will be "bcd".
///
/// # Notes
/// If start >= end, or indices are out of bounds, the result is an empty string.
/// This function operates on byte indices, so be careful with multi-byte UTF-8 characters.
#[bp_doc("# String Substring")]
#[bp_doc("Extracts a portion of a string from start to end index.")]
#[blueprint(type: NodeTypes::pure, category: "String", color: "#7ED321")]
pub fn string_substring(text: String, start: i64, end: i64) -> String {
    let start = start as usize;
    let end = end as usize;
    if start < text.len() && end <= text.len() && start <= end {
        text[start..end].to_string()
    } else {
        String::new()
    }
}

/// Repeats a string a specified number of times.
///
/// # Inputs
/// - `text`: The string to repeat
/// - `count`: The number of times to repeat the string (integer)
///
/// # Returns
/// A new string consisting of the input string repeated count times
///
/// # Example
/// If `text` is "abc" and `count` is 3, the output will be "abcabcabc".
/// If `count` is 0, the output will be an empty string.
///
/// # Notes
/// If count is negative, it is treated as zero. Use this function for string multiplication or pattern generation.
#[bp_doc("# String Repeat")]
#[bp_doc("Repeats a string a specified number of times.")]
#[blueprint(type: NodeTypes::pure, category: "String", color: "#7ED321")]
pub fn string_repeat(text: String, count: i64) -> String {
    text.repeat(count.max(0) as usize)
}

/// Removes whitespace from both ends of a string.
///
/// # Inputs
/// - `text`: The string to trim
///
/// # Returns
/// The trimmed string with no leading or trailing whitespace
///
/// # Example
/// If `text` is "  hello world  ", the output will be "hello world".
///
/// # Notes
/// Whitespace includes spaces, tabs, and newlines.
#[bp_doc("# String Trim")]
#[bp_doc("Removes leading and trailing whitespace from a string.")]
#[blueprint(type: NodeTypes::pure, category: "String", color: "#7ED321")]
pub fn string_trim(text: String) -> String {
    text.trim().to_string()
}

// =============================================================================
// Case Conversion
// =============================================================================

/// Converts a string to uppercase.
///
/// # Inputs
/// - `text`: The string to convert to uppercase
///
/// # Returns
/// A new string with all characters in uppercase
///
/// # Example
/// If `text` is "hello", the output will be "HELLO".
///
/// # Notes
/// Non-alphabetic characters are not affected.
#[bp_doc("# String Uppercase")]
#[bp_doc("Converts a string to uppercase.")]
#[blueprint(type: NodeTypes::pure, category: "String", color: "#7ED321")]
pub fn string_uppercase(text: String) -> String {
    text.to_uppercase()
}

/// Converts a string to lowercase.
///
/// # Inputs
/// - `text`: The input string to convert
///
/// # Returns
/// A new string with all characters in lowercase
///
/// # Example
/// If `text` is "Hello World!", the output will be "hello world!".
///
/// # Notes
/// The conversion uses Unicode-aware lowercasing. Non-alphabetic characters are unaffected.
#[bp_doc("# String Lowercase")]
#[bp_doc("Converts a string to lowercase.")]
#[blueprint(type: NodeTypes::pure, category: "String", color: "#7ED321")]
pub fn string_lowercase(text: String) -> String {
    text.to_lowercase()
}

// =============================================================================
// String Splitting and Joining
// =============================================================================

/// Splits a string by a delimiter.
///
/// # Inputs
/// - `text`: The string to split
/// - `delimiter`: The delimiter string to split by
///
/// # Returns
/// A vector of substrings resulting from the split
///
/// # Example
/// If `text` is "a,b,c" and `delimiter` is ",", the output will be
/// `["a", "b", "c"]`.
///
/// # Notes
/// If the delimiter is not found, the output will be a single-element vector containing the original string.
/// If the delimiter is an empty string, the split behavior follows Rust's default (splits at each character boundary).
#[bp_doc("# String Split")]
#[bp_doc("Splits a string into a vector of substrings by a delimiter.")]
#[blueprint(type: NodeTypes::pure, category: "String", color: "#7ED321")]
pub fn string_split(text: String, delimiter: String) -> Vec<String> {
    text.split(&delimiter).map(|s| s.to_string()).collect()
}

/// Joins a vector of strings with a separator.
///
/// # Inputs
/// - `strings`: The vector of strings to join
/// - `separator`: The separator string to insert between elements
///
/// # Returns
/// A single string with all elements of the vector joined by the separator
///
/// # Example
/// If `strings` is ["foo", "bar", "baz"] and `separator` is ",", the output will be "foo,bar,baz".
///
/// # Notes
/// If the vector is empty, the output will be an empty string. If the separator is empty, the strings are concatenated directly.
#[bp_doc("# String Join")]
#[bp_doc("Joins a vector of strings with a separator.")]
#[blueprint(type: NodeTypes::pure, category: "String", color: "#7ED321")]
pub fn string_join(strings: Vec<String>, separator: String) -> String {
    strings.join(&separator)
}

// =============================================================================
// String Formatting
// =============================================================================

/// Formats a string with a single placeholder.
///
/// # Inputs
/// - `template`: The template string containing a placeholder (e.g., "{}")
/// - `value`: The value to insert into the template
///
/// # Returns
/// The formatted string with the placeholder replaced by the value
///
/// # Example
/// If `template` is "Hello, {}!" and `value` is "Alice", the output will be "Hello, Alice!".
///
/// # Notes
/// This implementation uses Rust's `format!` macro. Only the first placeholder is replaced.
/// For more complex formatting, use additional format functions or extend the template.
#[bp_doc("# String Format")]
#[bp_doc("Formats a string by replacing placeholders with values.")]
#[blueprint(type: NodeTypes::pure, category: "String", color: "#7ED321")]
pub fn string_format(template: String, value: String) -> String {
    format!("{}", template.replacen("{}", &value, 1))
}

// =============================================================================
// Type Conversions - String to Other Types
// =============================================================================

/// Safely converts a string to an integer.
///
/// # Inputs
/// - `text`: The string to convert to an integer
///
/// # Returns
/// The parsed integer, or an error message if parsing fails
///
/// # Example
/// If `text` is "42", the output will be `Ok(42)`.
/// If `text` is "abc", the output will be `Err("Cannot convert 'abc' to integer")`.
///
/// # Notes
/// Leading and trailing whitespace is ignored. Only base-10 integers are supported.
#[bp_doc("# String to Int")]
#[bp_doc("Converts a string to an integer with error handling.")]
#[blueprint(type: NodeTypes::pure, category: "String/Conversion", color: "#7ED321")]
pub fn string_to_int(text: String) -> Result<i32, String> {
    match text.trim().parse::<i32>() {
        Ok(num) => Ok(num),
        Err(_) => Err(format!("Cannot convert '{}' to integer", text)),
    }
}

/// Safely converts a string to a number (floating point).
///
/// # Inputs
/// - `text`: The string to convert to a number
///
/// # Returns
/// The parsed number, or an error message if parsing fails
///
/// # Example
/// If `text` is "3.14", the output will be `Ok(3.14)`.
/// If `text` is "abc", the output will be `Err("Cannot convert 'abc' to number")`.
///
/// # Notes
/// Leading and trailing whitespace is ignored. Only base-10 floating-point numbers are supported.
#[bp_doc("# String to Number")]
#[bp_doc("Converts a string to a floating-point number with error handling.")]
#[blueprint(type: NodeTypes::pure, category: "String/Conversion", color: "#7ED321")]
pub fn string_to_number(text: String) -> Result<f32, String> {
    match text.trim().parse::<f32>() {
        Ok(num) => Ok(num),
        Err(_) => Err(format!("Cannot convert '{}' to number", text)),
    }
}

/// Safely converts a string to a boolean.
///
/// # Inputs
/// - `text`: The string to convert to a boolean
///
/// # Returns
/// The boolean value if conversion succeeds, or an error message if it fails
///
/// # Example
/// - Input: "true" → Output: Ok(true)
/// - Input: "no" → Output: Ok(false)
/// - Input: "maybe" → Output: Err("Cannot convert 'maybe' to boolean")
///
/// # Notes
/// Accepts common representations such as "true", "false", "1", "0", "yes", "no", "on", "off" (case-insensitive).
#[bp_doc("# String to Bool")]
#[bp_doc("Converts a string to a boolean value with error handling.")]
#[blueprint(type: NodeTypes::pure, category: "String/Conversion", color: "#7ED321")]
pub fn string_to_bool(text: String) -> Result<bool, String> {
    let text_lower = text.trim().to_lowercase();
    match text_lower.as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(format!("Cannot convert '{}' to boolean", text)),
    }
}

// =============================================================================
// Type Conversions - Other Types to String
// =============================================================================

/// Converts a number to its string representation.
///
/// # Inputs
/// - `number`: The number to convert
///
/// # Returns
/// The string representation of the input number
///
/// # Example
/// If `number` is 42, the output will be "42".
/// If `number` is 3.14, the output will be "3.14".
///
/// # Notes
/// The output uses the default formatting for the numeric type.
#[bp_doc("# Number to String")]
#[bp_doc("Converts a number to its string representation.")]
#[blueprint(type: NodeTypes::pure, category: "String/Conversion", color: "#7ED321")]
pub fn number_to_string(number: f64) -> String {
    number.to_string()
}

/// Converts a boolean value to its string representation.
///
/// # Inputs
/// - `value`: The boolean value to convert
///
/// # Returns
/// The string representation of the boolean ("true" or "false")
///
/// # Example
/// If `value` is true, the output will be "true". If false, the output will be "false".
///
/// # Notes
/// The output is always lowercase.
#[bp_doc("# Bool to String")]
#[bp_doc("Converts a boolean to its string representation.")]
#[blueprint(type: NodeTypes::pure, category: "String/Conversion", color: "#7ED321")]
pub fn bool_to_string(value: bool) -> String {
    value.to_string()
}
