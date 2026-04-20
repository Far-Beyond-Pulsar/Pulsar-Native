use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "project";
pub const OWNER: &str = "audio";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Audio", "Audio engine and mixer settings")
        .setting("audio_backend",
            SchemaEntry::new("Audio output backend", "auto")
                .label("Audio Backend").page("Audio")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Auto", "auto"),
                    DropdownOption::new("WASAPI (Windows)", "wasapi"),
                    DropdownOption::new("CoreAudio (macOS)", "coreaudio"),
                    DropdownOption::new("ALSA (Linux)", "alsa"),
                    DropdownOption::new("PulseAudio (Linux)", "pulse"),
                    DropdownOption::new("PipeWire (Linux)", "pipewire"),
                ]}))
        .setting("sample_rate",
            SchemaEntry::new("Audio output sample rate in Hz", "48000")
                .label("Sample Rate").page("Audio")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("44100 Hz", "44100"),
                    DropdownOption::new("48000 Hz", "48000"),
                    DropdownOption::new("96000 Hz", "96000"),
                ]}))
        .setting("buffer_size_frames",
            SchemaEntry::new("Audio buffer size in frames (lower = less latency, more CPU)", 512_i64)
                .label("Buffer Size (frames)").page("Audio")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("128 (very low latency)", "128"),
                    DropdownOption::new("256", "256"),
                    DropdownOption::new("512", "512"),
                    DropdownOption::new("1024", "1024"),
                    DropdownOption::new("2048", "2048"),
                ]}))
        .setting("master_volume",
            SchemaEntry::new("Master volume (0.0–1.0)", 1.0_f64)
                .label("Master Volume").page("Audio")
                .field_type(FieldType::Slider { min: 0.0, max: 1.0, step: 0.01 })
                .validator(Validator::float_range(0.0, 1.0)))
        .setting("music_volume",
            SchemaEntry::new("Background music volume", 0.8_f64)
                .label("Music Volume").page("Audio")
                .field_type(FieldType::Slider { min: 0.0, max: 1.0, step: 0.01 })
                .validator(Validator::float_range(0.0, 1.0)))
        .setting("sfx_volume",
            SchemaEntry::new("Sound effects volume", 1.0_f64)
                .label("SFX Volume").page("Audio")
                .field_type(FieldType::Slider { min: 0.0, max: 1.0, step: 0.01 })
                .validator(Validator::float_range(0.0, 1.0)))
        .setting("ambient_volume",
            SchemaEntry::new("Ambient / environmental sound volume", 0.7_f64)
                .label("Ambient Volume").page("Audio")
                .field_type(FieldType::Slider { min: 0.0, max: 1.0, step: 0.01 })
                .validator(Validator::float_range(0.0, 1.0)))
        .setting("voice_volume",
            SchemaEntry::new("Voice / dialogue volume", 1.0_f64)
                .label("Voice Volume").page("Audio")
                .field_type(FieldType::Slider { min: 0.0, max: 1.0, step: 0.01 })
                .validator(Validator::float_range(0.0, 1.0)))
        .setting("spatial_audio",
            SchemaEntry::new("Enable 3D positional audio processing", true)
                .label("Spatial Audio").page("Audio")
                .field_type(FieldType::Checkbox))
        .setting("reverb_enabled",
            SchemaEntry::new("Enable global reverb convolution for environmental sound", false)
                .label("Reverb").page("Audio")
                .field_type(FieldType::Checkbox))
        .setting("hrtf",
            SchemaEntry::new("Use Head-Related Transfer Function for headphone 3D audio", false)
                .label("HRTF (Headphones)").page("Audio")
                .field_type(FieldType::Checkbox))
        .setting("max_simultaneous_sounds",
            SchemaEntry::new("Maximum number of sounds playing at once before eviction", 64_i64)
                .label("Max Simultaneous Sounds").page("Audio")
                .field_type(FieldType::NumberInput { min: Some(8.0), max: Some(512.0), step: Some(8.0) })
                .validator(Validator::int_range(8, 512)))
        .setting("mute_on_focus_loss",
            SchemaEntry::new("Mute all audio when the game window loses focus", false)
                .label("Mute on Focus Loss").page("Audio")
                .field_type(FieldType::Checkbox))
        .setting("spatial_audio_enabled",
            SchemaEntry::new("Enable 3D positional audio spatialization", true)
                .label("3D Spatial Audio").page("Audio")
                .field_type(FieldType::Checkbox))
        .setting("hrtf_enabled",
            SchemaEntry::new("Use Head-Related Transfer Function (HRTF) for realistic headphone spatialization", false)
                .label("HRTF (Headphone Spatialization)").page("Audio")
                .field_type(FieldType::Checkbox))
        .setting("reverb_enabled",
            SchemaEntry::new("Enable environmental reverb and convolution on audio sources", true)
                .label("Reverb").page("Audio")
                .field_type(FieldType::Checkbox))
        .setting("occlusion_enabled",
            SchemaEntry::new("Occlude sounds that pass through geometry using raycasts", false)
                .label("Audio Occlusion").page("Audio")
                .field_type(FieldType::Checkbox))
        .setting("voice_channels",
            SchemaEntry::new("Maximum number of simultaneous voice/sound channels", 64_i64)
                .label("Voice Channels").page("Audio")
                .field_type(FieldType::NumberInput { min: Some(8.0), max: Some(512.0), step: Some(8.0) })
                .validator(Validator::int_range(8, 512)))
        .setting("doppler_scale",
            SchemaEntry::new("Doppler effect intensity multiplier (0.0 = off, 1.0 = realistic)", 0.0_f64)
                .label("Doppler Scale").page("Audio")
                .field_type(FieldType::Slider { min: 0.0, max: 3.0, step: 0.1 })
                .validator(Validator::float_range(0.0, 3.0)))
        .setting("distance_model",
            SchemaEntry::new("Falloff model for 3D audio attenuation", "inverse")
                .label("Distance Attenuation Model").page("Audio")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Inverse", "inverse"),
                    DropdownOption::new("Linear", "linear"),
                    DropdownOption::new("Exponential", "exponential"),
                    DropdownOption::new("None", "none"),
                ]})
                .validator(Validator::string_one_of(["inverse", "linear", "exponential", "none"])))
        .setting("rolloff_factor",
            SchemaEntry::new("Rolloff factor for the distance attenuation model", 1.0_f64)
                .label("Rolloff Factor").page("Audio")
                .field_type(FieldType::Slider { min: 0.0, max: 10.0, step: 0.1 })
                .validator(Validator::float_range(0.0, 10.0)))
        .setting("streaming_threshold_kb",
            SchemaEntry::new("Audio files larger than this value (KB) are streamed from disk", 256_i64)
                .label("Streaming Threshold (KB)").page("Audio")
                .field_type(FieldType::NumberInput { min: Some(64.0), max: Some(8192.0), step: Some(64.0) })
                .validator(Validator::int_range(64, 8192)))
        .setting("mix_presets_file",
            SchemaEntry::new("Path to the audio mix presets/busses configuration file", "config/audio_mix.toml")
                .label("Mix Presets File").page("Audio")
                .field_type(FieldType::TextInput { placeholder: Some("config/audio_mix.toml".into()), multiline: false }))
        .setting("duck_on_voice",
            SchemaEntry::new("Automatically duck (reduce) background music volume during voice lines", true)
                .label("Duck on Voice").page("Audio")
                .field_type(FieldType::Checkbox))
        .setting("duck_amount_db",
            SchemaEntry::new("Amount to reduce background music volume during ducking in dB", -12.0_f64)
                .label("Duck Amount (dB)").page("Audio")
                .field_type(FieldType::Slider { min: -40.0, max: 0.0, step: 1.0 })
                .validator(Validator::float_range(-40.0, 0.0)));

    let _ = cfg.register(NS, OWNER, schema);
}
