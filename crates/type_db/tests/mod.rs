#[cfg(test)]
mod tests {
    use super::*;
    use plugin_editor_api::FileTypeId;

    // # TypeDatabase Tests
    //
    // This module contains unit and performance tests for the TypeDatabase.
    // It covers registration, lookup, removal, edge cases, and basic performance/concurrency.
    //
    // Performance tests are not strict benchmarks, but will fail if operations are unreasonably slow.

    use std::time::Instant;
    use std::thread;
    use std::sync::Arc;

    #[test]
    fn test_register_and_get() {
        let mut db = TypeDatabase::new();
        let file_type = FileTypeId::new("struct");
        let id = db.register("Vector3", Some("Math".to_string()), None, None, file_type.clone(), None, None, None);

        let type_info = db.get(id).unwrap();
        assert_eq!(type_info.name, "Vector3");
        assert_eq!(type_info.category, Some("Math".to_string()));
        assert_eq!(type_info.file_type_id, file_type);
    }

    #[test]
    fn test_search() {
        let mut db = TypeDatabase::new();
        let file_type = FileTypeId::new("struct");
        db.register("Vector2", Some("Math".to_string()), None, None, file_type.clone(), None, None, None);
        db.register("Vector3", Some("Math".to_string()), None, None, file_type.clone(), None, None, None);
        db.register("String", Some("Primitives".to_string()), None, None, file_type, None, None, None);

        let results = db.search("vec");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_fuzzy_search() {
        let mut db = TypeDatabase::new();
        let file_type = FileTypeId::new("struct");
        db.register_simple("PlayerController", file_type.clone());
        db.register_simple("EnemyController", file_type.clone());
        db.register_simple("GameManager", file_type);

        let results = db.search_fuzzy("pc");
        assert!(!results.is_empty());
        assert_eq!(results[0].name, "PlayerController");
    }

    #[test]
    fn test_category_lookup() {
        let mut db = TypeDatabase::new();
        let file_type = FileTypeId::new("struct");
        db.register("Vector2", Some("Math".to_string()), None, None, file_type.clone(), None, None, None);
        db.register("Vector3", Some("Math".to_string()), None, None, file_type.clone(), None, None, None);
        db.register("String", Some("Primitives".to_string()), None, None, file_type, None, None, None);

        let math_types = db.get_by_category("math");
        assert_eq!(math_types.len(), 2);
    }

    #[test]
    fn test_unregister() {
        let mut db = TypeDatabase::new();
        let file_type = FileTypeId::new("struct");
        let id = db.register("TestType", Some("TestCat".to_string()), Some("desc".to_string()), None, file_type, None, None, None);
        assert!(db.get(id).is_some());
        let removed = db.unregister(id);
        assert!(removed.is_some());
        assert!(db.get(id).is_none());
        // Unregistering again should return None
        assert!(db.unregister(id).is_none());
    }

    #[test]
    fn test_clear_and_is_empty() {
        let mut db = TypeDatabase::new();
        let file_type = FileTypeId::new("struct");
        db.register_simple("A", file_type.clone());
        db.register_simple("B", file_type);
        assert!(!db.is_empty());
        db.clear();
        assert!(db.is_empty());
        assert_eq!(db.len(), 0);
    }

    #[test]
    fn test_duplicate_names() {
        let mut db = TypeDatabase::new();
        let file_type = FileTypeId::new("struct");
        let id1 = db.register_simple("DupType", file_type.clone());
        let id2 = db.register_simple("DupType", file_type);
        let found = db.get_by_name("DupType");
        assert_eq!(found.len(), 2);
        assert!(found.iter().any(|t| t.id == id1));
        assert!(found.iter().any(|t| t.id == id2));
    }

    #[test]
    fn test_case_insensitive_lookup() {
        let mut db = TypeDatabase::new();
        let file_type = FileTypeId::new("struct");
        db.register("CaseType", Some("Category".to_string()), None, None, file_type, None, None, None);
        let found = db.get_by_name("casetype");
        assert_eq!(found.len(), 1);
        let found_cat = db.get_by_category("category");
        assert_eq!(found_cat.len(), 1);
    }

    #[test]
    fn test_all_returns_all_types() {
        let mut db = TypeDatabase::new();
        let file_type = FileTypeId::new("struct");
        db.register_simple("A", file_type.clone());
        db.register_simple("B", file_type);
        let all = db.all();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_search_no_results() {
        let mut db = TypeDatabase::new();
        let file_type = FileTypeId::new("struct");
        db.register_simple("Alpha", file_type);
        let results = db.search("Beta");
        assert!(results.is_empty());
    }

    #[test]
    fn test_fuzzy_search_no_results() {
        let mut db = TypeDatabase::new();
        let file_type = FileTypeId::new("struct");
        db.register_simple("Alpha", file_type);
        let results = db.search_fuzzy("zzz");
        assert!(results.is_empty());
    }

    #[test]
    fn test_large_insert_performance() {
        let mut db = TypeDatabase::new();
        let file_type = FileTypeId::new("struct");
        let count = 10_000;
        let start = Instant::now();
        for i in 0..count {
            db.register(format!("Type{}", i), Some("Perf".to_string()), None, None, file_type.clone(), None, None, None);
        }
        let duration = start.elapsed();
        assert_eq!(db.len(), count as usize);
        // Should be reasonably fast (arbitrary: < 1s)
        assert!(duration.as_secs_f32() < 1.0, "Insert took too long: {:?}", duration);
    }

    #[test]
    fn test_concurrent_inserts() {
        use std::sync::Mutex;
        let db = Arc::new(Mutex::new(TypeDatabase::new()));
        let threads: Vec<_> = (0..8).map(|t| {
            let db = db.clone();
            thread::spawn(move || {
                let file_type = FileTypeId::new("struct");
                for i in 0..2_000 {
                    let mut db = db.lock().unwrap();
                    db.register(format!("T{}_{}", t, i), Some("Cat".to_string()), None, None, file_type.clone(), None, None, None);
                }
            })
        }).collect();
        for th in threads { th.join().unwrap(); }
        let db = db.lock().unwrap();
        assert_eq!(db.len(), 16_000);
    }

    #[test]
    fn test_concurrent_reads() {
        let mut db = TypeDatabase::new();
        let file_type = FileTypeId::new("struct");
        for i in 0..1000 {
            db.register(format!("Type{}", i), Some("Cat".to_string()), None, None, file_type.clone(), None, None, None);
        }
        let db = Arc::new(db);
        let threads: Vec<_> = (0..4).map(|_| {
            let db = db.clone();
            thread::spawn(move || {
                for i in 0..1000 {
                    let _ = db.get_by_name(&format!("Type{}", i));
                }
            })
        }).collect();
        for th in threads { th.join().unwrap(); }
    }

    #[test]
    fn test_file_path_lookup() {
        let mut db = TypeDatabase::new();
        let path = std::path::PathBuf::from("/test/file.rs");
        let file_type = FileTypeId::new("struct");
        let id = db.register(
            "TestType",
            None,
            None,
            Some(path.clone()),
            file_type,
            None,
            None,
            None,
        );

        let found = db.get_by_path(&path);
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, id);

        let removed = db.unregister_by_path(&path);
        assert!(removed.is_some());
        assert!(db.get_by_path(&path).is_none());
    }

    #[test]
    fn test_get_by_file_type() {
        let mut db = TypeDatabase::new();
        let struct_type = FileTypeId::new("struct");
        let enum_type = FileTypeId::new("enum");
        let trait_type = FileTypeId::new("trait");
        
        db.register_simple("Struct1", struct_type.clone());
        db.register_simple("Struct2", struct_type.clone());
        db.register_simple("Enum1", enum_type.clone());
        db.register_simple("Trait1", trait_type.clone());

        let structs = db.get_by_file_type(&struct_type);
        assert_eq!(structs.len(), 2);

        let enums = db.get_by_file_type(&enum_type);
        assert_eq!(enums.len(), 1);

        let traits = db.get_by_file_type(&trait_type);
        assert_eq!(traits.len(), 1);
    }

    #[test]
    fn test_count_by_file_type() {
        let mut db = TypeDatabase::new();
        let struct_type = FileTypeId::new("struct");
        let enum_type = FileTypeId::new("enum");
        let trait_type = FileTypeId::new("trait");
        
        db.register_simple("Struct1", struct_type.clone());
        db.register_simple("Struct2", struct_type.clone());
        db.register_simple("Enum1", enum_type.clone());

        assert_eq!(db.count_by_file_type(&struct_type), 2);
        assert_eq!(db.count_by_file_type(&enum_type), 1);
        assert_eq!(db.count_by_file_type(&trait_type), 0);
    }
}