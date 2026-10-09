//! The generated declarations and sample classes under the real TypeScript
//! compiler.
//!
//! `declarations()` is only worth anything if TypeScript accepts it, and the
//! subset only worth anything if what this compiler accepts, `tsc` accepts
//! too. This runs `tsc --noEmit` (strict) over the declarations and every
//! sample class. It needs Node and network (npx fetches `typescript`), so it
//! is ignored by default: `just check-typescript-declarations`.

use std::process::Command;

use pulsar_script_ts::declarations;
use pulsar_script_vm::NativeRegistry;

use pulsar_script_math as _;

const SAMPLES: &[(&str, &str)] = &[
    (
        "Door",
        r#"
export default class Door extends ScriptClass {
    speed: number = 2.5;
    opened: int = 0;
    position: Vec3 = Vec3.new_(0, 0, 0);
    label: string = "door";
    @renamedFrom("hp") health: int = 10;

    async open(): Promise<void> {
        await wait(1.5);
        this.opened += 1;
        this.position = this.position.add(Vec3.new_(0, this.speed, 0));
    }

    tick(delta: number): void {
        let height: number = this.position.y;
        for (let i = 0; i < 3; i++) { height += math.sqrt(i as number); }
        if (height > 2 && this.opened > 0) { this.label = "tall"; }
    }

    classify(x: int): string { return x < 0 ? "negative" : "positive"; }
    me(): Entity { return this.entity; }
    name_length(s: string): int { return string_.len(s); }
}
"#,
    ),
    (
        "Beacon",
        r#"
export default class Beacon extends ScriptClass {
    log: string = "";
    flip: boolean = false;
    async on_fire(): Promise<void> {
        this.log += this.flip ? "a" : "b";
        this.flip = !this.flip;
        await wait(0.5);
        this.log += "d";
    }
}
"#,
    ),
];

#[test]
#[ignore = "needs Node and network: runs tsc over the generated declarations"]
fn declarations_and_sample_classes_type_check_with_tsc() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("pulsar.d.ts"),
        declarations(&NativeRegistry::with_engine_natives()),
    )
    .unwrap();
    let mut files = vec!["pulsar.d.ts".to_owned()];
    for (name, source) in SAMPLES {
        std::fs::write(dir.path().join(format!("{name}.ts")), source).unwrap();
        files.push(format!("{name}.ts"));
    }
    let tsconfig = serde_json::json!({
        "compilerOptions": {
            "strict": true,
            "noEmit": true,
            "target": "ES2022",
            "lib": ["ES2022"],
            "types": [],
            "skipLibCheck": false,
            "noUnusedLocals": false,
        },
        "files": files,
    });
    std::fs::write(dir.path().join("tsconfig.json"), tsconfig.to_string()).unwrap();

    let npx = if cfg!(windows) { "npx.cmd" } else { "npx" };
    let output = Command::new(npx)
        .args(["--yes", "-p", "typescript@5", "tsc", "-p", "tsconfig.json"])
        .current_dir(dir.path())
        .output()
        .expect("npx is available");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "tsc rejected the declarations or a sample class:\n{text}"
    );
}
