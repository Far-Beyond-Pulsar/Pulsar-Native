#[cfg(test)]
mod tests {
    use super::super::*;
    #[test]
    fn test_engine_error_display() {
        let err = EngineError::Config("bad config".into());
        assert_eq!(format!("{}", err), "Configuration error: bad config");
    }

    #[test]
    fn test_unwrap_or_warn_macro_success() {
        let val: Result<u32, &str> = Ok(5);
        let res = unwrap_or_warn!(val);
        assert_eq!(res, 5);
    }

    #[test]
    fn test_unwrap_or_warn_macro_failure() {
        let val: Result<u32, &str> = Err("oops");
        let res = unwrap_or_warn!(val);
        assert_eq!(res, Default::default()); // u32 default is 0
    }
}
