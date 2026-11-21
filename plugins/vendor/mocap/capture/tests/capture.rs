use mocap_capture::osc::{encode_bundle, encode_message, parse_packet, OscArg};
use mocap_capture::*;
use mocap_format::{read_take, TakeMeta};
use std::time::Duration;

fn float(v: f32) -> OscArg {
    OscArg::Float(v)
}

fn bone(name: &str, p: [f32; 3], q: [f32; 4]) -> Vec<u8> {
    let mut args = vec![OscArg::Str(name.to_owned())];
    args.extend(p.iter().chain(q.iter()).map(|v| float(*v)));
    encode_message("/VMC/Ext/Bone/Pos", &args)
}

#[test]
fn osc_messages_and_bundles_round_trip() {
    let single = encode_message("/a/b", &[OscArg::Str("hi".into()), OscArg::Int(7), float(0.5)]);
    let parsed = parse_packet(&single);
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].string(0), Some("hi"));
    assert_eq!(parsed[0].float(1), Some(7.0));
    assert_eq!(parsed[0].float(2), Some(0.5));
    let bundle = encode_bundle(&[single.clone(), encode_message("/c", &[])]);
    assert_eq!(parse_packet(&bundle).len(), 2);
}

#[test]
fn osc_garbage_never_panics() {
    for packet in [&b""[..], b"/", b"/x\0\0,f\0\0", b"#bundle\0", b"#bundle\0\0\0\0\0\0\0\0\x01\xff\xff\xff\xff"] {
        let _ = parse_packet(packet);
    }
    let mut noise = vec![0u8; 512];
    for (i, b) in noise.iter_mut().enumerate() {
        *b = (i * 31 % 251) as u8;
    }
    let _ = parse_packet(&noise);
}

#[test]
fn vmc_decoder_fills_channels_and_commits_on_apply() {
    let rig = vrm_humanoid();
    let index = RigIndex::new(&rig);
    let mut decoder = VmcDecoder::new(&rig);
    let q = [0.0, 0.5, 0.0, 0.5];
    for packet in [bone("Hips", [0.0, 0.9, 0.1], q), bone("Head", [0.0; 3], q)] {
        for message in parse_packet(&packet) {
            assert!(!decoder.apply(&message));
        }
    }
    let blend = encode_message("/VMC/Ext/Blend/Val", &[OscArg::Str("Blink".into()), float(1.0)]);
    for message in parse_packet(&blend) {
        decoder.apply(&message);
    }
    let apply = encode_message("/VMC/Ext/Blend/Apply", &[]);
    assert!(parse_packet(&apply).iter().any(|m| decoder.apply(m)));
    let frame = decoder.snapshot(1.0);
    let head = index.column("Head.rotation").expect("head");
    assert_eq!(&frame.values[head..head + 4], &q);
    let hips = index.column("Hips.position").expect("hips");
    assert_eq!(frame.values[hips + 1], 0.9);
    assert_eq!(frame.values[index.column("blend.Blink").expect("blink")], 1.0);
    let untouched = index.column("LeftHand.rotation").expect("hand");
    assert!(frame.values[untouched].is_nan(), "an unreported bone is a gap, not zero");
}

#[test]
fn a_frame_missing_a_component_is_ignored() {
    let rig = vrm_humanoid();
    let mut decoder = VmcDecoder::new(&rig);
    let short = encode_message("/VMC/Ext/Bone/Pos", &[OscArg::Str("Head".into()), float(0.0)]);
    for m in parse_packet(&short) {
        decoder.apply(&m);
    }
    assert!(!decoder.has_pending());
}

#[test]
fn the_vrm_rig_is_valid_and_indexed() {
    let rig = vrm_humanoid();
    rig.validate().expect("valid");
    // Root + 25 body bones + 30 finger bones.
    assert_eq!(rig.joints.len(), 56);
    let index = RigIndex::new(&rig);
    assert_eq!(index.width(), rig.columns());
    assert!(index.column("LeftThumbDistal.rotation").is_some());
}

#[test]
fn the_pose_bus_drops_the_oldest_and_counts() {
    let bus = PoseBus::new(3);
    for i in 0..5 {
        bus.push(PoseFrame { sequence: i, time_s: i as f64, values: vec![] });
    }
    let mut out = Vec::new();
    bus.drain(&mut out);
    assert_eq!(out.iter().map(|f| f.sequence).collect::<Vec<_>>(), vec![2, 3, 4]);
    assert_eq!(bus.stats(), BusStats { pushed: 5, dropped: 2 });
    assert_eq!(bus.latest().map(|f| f.sequence), Some(4));
}

#[test]
fn the_video_mailbox_only_offers_newer_frames() {
    let mailbox = VideoMailbox::new();
    assert!(mailbox.latest_if_newer(0).is_none());
    let frame = |s| VideoFrame {
        width: 2,
        height: 1,
        format: PixelFormat::Rgba8,
        data: vec![0u8; 8].into(),
        sequence: s,
        time_s: 0.0,
    };
    mailbox.put(frame(1));
    let (seen, got) = mailbox.latest_if_newer(0).expect("a frame");
    assert!(got.is_well_formed());
    assert!(mailbox.latest_if_newer(seen).is_none());
    mailbox.put(frame(2));
    assert_eq!(mailbox.latest_if_newer(seen).map(|(_, f)| f.sequence), Some(2));
}

fn synthetic_frames(fps: f32, seconds: f64) -> Vec<PoseFrame> {
    let index = RigIndex::new(&vrm_humanoid());
    (0..(f64::from(fps) * seconds) as u64)
        .map(|i| PoseFrame {
            sequence: i,
            time_s: i as f64 / f64::from(fps),
            values: synthetic_pose(&index, i, fps),
        })
        .collect()
}

#[test]
fn the_recorder_resamples_120_hz_to_a_60_fps_take() {
    let rig = vrm_humanoid();
    let meta = TakeMeta::new("resample", 60, 1);
    let mut recorder = Recorder::new(Vec::new(), &meta, &rig, 30).expect("recorder");
    for frame in synthetic_frames(120.0, 2.0) {
        recorder.push(&frame).expect("push");
    }
    let stats = recorder.stats();
    assert_eq!(stats.gap_frames, 0);
    let (take, report) = read_take(&recorder.finish().expect("finish")).expect("read");
    assert!(report.is_clean());
    assert!((119..=120).contains(&take.frame_count()), "{} frames", take.frame_count());
    let blink = rig.column_offsets()[rig.channel_index("blend.Blink").expect("blink")];
    assert!(take.columns[blink].iter().any(|v| *v > 0.5), "the blink was captured");
}

#[test]
fn a_silent_source_records_gaps_not_inventions() {
    let rig = vrm_humanoid();
    let meta = TakeMeta::new("gap", 60, 1);
    let mut recorder = Recorder::new(Vec::new(), &meta, &rig, 30).expect("recorder");
    let frames = synthetic_frames(60.0, 3.0);
    for (i, frame) in frames.iter().enumerate() {
        // Two seconds of silence in the middle.
        if !(30..150).contains(&i) {
            recorder.push(frame).expect("push");
        }
    }
    assert!(recorder.stats().gap_frames > 100, "{:?}", recorder.stats());
    let (take, _) = read_take(&recorder.finish().expect("finish")).expect("read");
    assert!(take.columns[0][60].is_nan());
    assert!(take.columns[0][5].is_finite());
}

#[test]
fn the_processor_smooths_jitter_and_keeps_rotations_unit() {
    let rig = vrm_humanoid();
    let index = RigIndex::new(&rig);
    let mut processor = Processor::new(&rig, FilterSettings::default());
    let head = index.column("Head.rotation").expect("head");
    let mut worst: f32 = 0.0;
    for i in 0..120u64 {
        let jitter = if i % 2 == 0 { 0.02 } else { -0.02 };
        let mut frame = PoseFrame {
            sequence: i,
            time_s: i as f64 / 60.0,
            values: synthetic_pose(&index, 0, 60.0),
        };
        frame.values[head] = jitter;
        processor.process(&mut frame);
        let q = &frame.values[head..head + 4];
        let length: f32 = q.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((length - 1.0).abs() < 1e-4);
        if i > 60 {
            worst = worst.max(frame.values[head].abs());
        }
    }
    assert!(worst < 0.015, "jitter damped to {worst}");
}

#[test]
fn calibration_makes_the_neutral_pose_the_identity() {
    let rig = vrm_humanoid();
    let index = RigIndex::new(&rig);
    let mut neutral = PoseFrame { sequence: 0, time_s: 0.0, values: synthetic_pose(&index, 0, 60.0) };
    let head = index.column("Head.rotation").expect("head");
    neutral.values[head..head + 4].copy_from_slice(&axis_angle([0.0, 1.0, 0.0], 0.5));
    let calibration = calibrate_neutral(&rig, &neutral, 1.0);
    assert!(calibration.scale > 0.9 && calibration.scale < 1.1);
    let mut again = neutral.clone();
    apply_calibration(&rig, &calibration, &mut again);
    let q = &again.values[head..head + 4];
    assert!(q[0].abs() < 1e-5 && q[1].abs() < 1e-5 && (q[3].abs() - 1.0).abs() < 1e-5, "{q:?}");
}

#[test]
fn a_synthetic_source_streams_through_the_bus_and_stops() {
    let bus = PoseBus::new(512);
    let sink = SourceSink { poses: bus.clone(), video: VideoMailbox::new(), clock: SessionClock::start() };
    let mut source = SyntheticSource::new(120.0);
    source.start(sink.clone()).expect("start");
    assert!(matches!(source.start(sink), Err(CaptureError::AlreadyRunning)));
    let mut frames = Vec::new();
    bus.wait_drain(Duration::from_secs(2), &mut frames);
    std::thread::sleep(Duration::from_millis(300));
    bus.drain(&mut frames);
    assert!(frames.len() > 10, "{} frames", frames.len());
    assert!(frames.windows(2).all(|w| w[1].time_s >= w[0].time_s));
    assert_eq!(source.status().state, SourceState::Live);
    source.stop();
    assert_eq!(source.status().state, SourceState::Idle);
    let after = bus.stats().pushed;
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(bus.stats().pushed, after, "nothing is delivered after stop");
}

#[test]
fn a_vmc_source_receives_udp_and_a_busy_port_is_an_error() {
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").expect("probe");
    let port = socket.local_addr().expect("addr").port();
    drop(socket);
    let bus = PoseBus::new(64);
    let sink = SourceSink { poses: bus.clone(), video: VideoMailbox::new(), clock: SessionClock::start() };
    let mut source = VmcSource::new(port);
    source.start(sink.clone()).expect("start");
    let mut second = VmcSource::new(port);
    assert!(matches!(second.start(sink), Err(CaptureError::Open { .. })));

    let sender = std::net::UdpSocket::bind("127.0.0.1:0").expect("sender");
    let packet = encode_bundle(&[
        bone("Head", [0.0; 3], [0.0, 0.0, 0.0, 1.0]),
        encode_message("/VMC/Ext/Blend/Apply", &[]),
    ]);
    let mut frames = Vec::new();
    for _ in 0..50 {
        sender.send_to(&packet, ("127.0.0.1", port)).expect("send");
        if bus.wait_drain(Duration::from_millis(40), &mut frames) > 0 {
            break;
        }
    }
    source.stop();
    assert!(!frames.is_empty(), "a VMC frame arrived");
}

#[test]
fn forward_kinematics_stands_the_figure_up_and_swings_the_arm() {
    let rig = vrm_humanoid();
    let index = RigIndex::new(&rig);
    let skeleton = Skeleton::new(&rig);
    let joint = |name: &str| rig.joints.iter().position(|j| j.name == name).expect(name);
    let rest = skeleton.joints(&synthetic_pose(&index, 0, 60.0));
    let head = rest[joint("Head")];
    assert!(head[1] > 1.4 && head[1] < 1.9, "head height {}", head[1]);
    assert!(rest[joint("LeftHand")][0] > 0.4, "the left hand is out to +X in the T-pose");
    assert!(rest[joint("RightHand")][0] < -0.4);
    assert!(rest[joint("LeftFoot")][1] < 0.2, "feet near the floor");
    assert_eq!(skeleton.bones().len(), rig.joints.len() - 1);
    // A quarter-cycle later the arms have swung: the hand moved.
    let swung = skeleton.joints(&synthetic_pose(&index, 15, 60.0));
    let moved: f32 = (0..3).map(|a| (swung[joint("LeftHand")][a] - rest[joint("LeftHand")][a]).abs()).sum();
    assert!(moved > 0.05, "hand moved {moved}");
    // A gap is treated as the rest pose, never a NaN position.
    let blank = skeleton.joints(&vec![f32::NAN; rig.columns()]);
    assert!(blank.iter().all(|p| p.iter().all(|v| v.is_finite())));
}

fn imu_packet(quat: [f32; 4]) -> Vec<u8> {
    encode_message("/imu/quat", &quat.map(OscArg::Float))
}

#[test]
fn merging_one_rig_keeps_its_names_and_several_are_namespaced() {
    let vrm = vrm_humanoid();
    let imu = sensor_rig(&OscSensorSource::imu(1).descriptor().rig.map_or(Vec::new(), |_| {
        vec![SensorChannel::new("imu.rotation", "/imu/quat", mocap_format::ChannelKind::Rotation4)]
    }));
    let (single, ranges) = merge_rigs(&[("a".to_owned(), &vrm)]);
    assert_eq!(single, vrm);
    assert_eq!(ranges, vec![0..vrm.columns()]);

    let (merged, ranges) = merge_rigs(&[("vmc:1".to_owned(), &vrm), ("vmc:2".to_owned(), &vrm), ("sensor:9".to_owned(), &imu)]);
    merged.validate().expect("a merged rig is valid");
    assert_eq!(merged.columns(), vrm.columns() * 2 + imu.columns());
    assert_eq!(ranges[1].start, vrm.columns());
    assert!(merged.channel_index("vmc:2/Hips.rotation").is_some());
    assert!(merged.channel_index("sensor:9/imu.rotation").is_some());
    // The second figure's hips parent is its own root, not the first's.
    let hips2 = merged.joints.iter().position(|j| j.name == "vmc:2/Hips").expect("hips");
    let root2 = merged.joints[hips2].parent.expect("parent");
    assert_eq!(merged.joints[usize::from(root2)].name, "vmc:2/Root");
}

#[test]
fn the_mixer_merges_sources_holds_the_slower_and_gaps_the_silent() {
    let vrm = vrm_humanoid();
    let imu_channels = vec![SensorChannel::new("imu.rotation", "/imu/quat", mocap_format::ChannelKind::Rotation4)];
    let imu = sensor_rig(&imu_channels);
    let (fast, slow) = (PoseBus::new(64), PoseBus::new(64));
    let (mut mixer, rig) = Mixer::new(vec![
        MixInput { id: "fig".into(), rig: vrm.clone(), bus: fast.clone() },
        MixInput { id: "imu".into(), rig: imu.clone(), bus: slow.clone() },
    ]);
    let index = RigIndex::new(&rig);
    let imu_col = index.column("imu/imu.rotation").expect("imu column");
    let hips_col = index.column("fig/Hips.position").expect("hips column");
    let pose = RigIndex::new(&vrm);
    slow.push(PoseFrame { sequence: 1, time_s: 0.0, values: vec![0.0, 0.0, 0.0, 1.0] });
    fast.push(PoseFrame { sequence: 1, time_s: 0.01, values: synthetic_pose(&pose, 0, 100.0) });
    fast.push(PoseFrame { sequence: 2, time_s: 0.02, values: synthetic_pose(&pose, 1, 100.0) });
    // Much later, only the fast source speaks: the IMU is now stale.
    fast.push(PoseFrame { sequence: 3, time_s: 1.0, values: synthetic_pose(&pose, 100, 100.0) });
    let mut out = Vec::new();
    mixer.pump(&mut out);
    assert_eq!(out.len(), 4);
    assert!(out.windows(2).all(|w| w[0].time_s <= w[1].time_s));
    assert!(out[0].values[hips_col].is_nan(), "before the figure speaks it is a gap");
    assert_eq!(out[1].values[imu_col + 3], 1.0, "the IMU's last value is held");
    assert!(out[1].values[hips_col + 1].is_finite());
    assert!(out[3].values[imu_col].is_nan(), "a silent source is a gap, not stale data");
    assert!(out[3].values[hips_col + 1].is_finite());
}

#[test]
fn a_sensor_decoder_maps_addresses_and_ignores_short_messages() {
    let channels = vec![
        SensorChannel::new("imu.rotation", "/imu/quat", mocap_format::ChannelKind::Rotation4),
        SensorChannel::new("pressure", "/p", mocap_format::ChannelKind::Scalar),
    ];
    let mut decoder = SensorDecoder::new(&channels);
    for m in parse_packet(&imu_packet([0.0, 0.0, 0.0, 1.0])) {
        decoder.apply(&m);
    }
    for m in parse_packet(&encode_message("/imu/quat", &[OscArg::Float(1.0)])) {
        decoder.apply(&m);
    }
    for m in parse_packet(&encode_message("/unrelated", &[OscArg::Float(5.0)])) {
        decoder.apply(&m);
    }
    let frame = decoder.snapshot(0.5);
    assert_eq!(&frame.values[..4], &[0.0, 0.0, 0.0, 1.0]);
    assert!(frame.values[4].is_nan(), "a channel never sent stays a gap");
}

#[test]
fn an_osc_sensor_source_streams_over_udp() {
    let probe = std::net::UdpSocket::bind("127.0.0.1:0").expect("probe");
    let port = probe.local_addr().expect("addr").port();
    drop(probe);
    let bus = PoseBus::new(64);
    let sink = SourceSink { poses: bus.clone(), video: VideoMailbox::new(), clock: SessionClock::start() };
    let mut source = OscSensorSource::imu(port);
    assert_eq!(source.descriptor().kind, SourceKind::Sensor);
    source.start(sink).expect("start");
    let sender = std::net::UdpSocket::bind("127.0.0.1:0").expect("sender");
    let mut frames = Vec::new();
    for _ in 0..50 {
        sender.send_to(&imu_packet([0.0, 0.0, 0.0, 1.0]), ("127.0.0.1", port)).expect("send");
        if bus.wait_drain(Duration::from_millis(40), &mut frames) > 0 {
            break;
        }
    }
    source.stop();
    assert!(!frames.is_empty(), "a sensor frame arrived");
}

#[test]
fn several_figures_in_one_rig_stand_apart() {
    let vrm = vrm_humanoid();
    let (rig, _) = merge_rigs(&[("a".to_owned(), &vrm), ("b".to_owned(), &vrm)]);
    let skeleton = Skeleton::new(&rig);
    let index = RigIndex::new(&rig);
    let values = vec![f32::NAN; rig.columns()];
    let _ = index;
    let joints = skeleton.joints(&values);
    let hips = |name: &str| rig.joints.iter().position(|j| j.name == name).expect(name);
    let gap = (joints[hips("b/Hips")][0] - joints[hips("a/Hips")][0]).abs();
    assert!(gap > 1.0, "figures are {gap} m apart");
    assert!(joints[hips("a/Head")][1] > 1.4, "names with a namespace still get rest offsets");
}

#[test]
fn numbered_synthetic_sources_have_distinct_ids() {
    let (a, b) = (SyntheticSource::numbered(60.0, 1), SyntheticSource::numbered(60.0, 2));
    assert_ne!(a.descriptor().id, b.descriptor().id);
}

fn solid_frame(width: u32, height: u32, rgba: [u8; 4], sequence: u64, time_s: f64) -> VideoFrame {
    let data: Vec<u8> = (0..width * height).flat_map(|_| rgba).collect();
    VideoFrame { width, height, format: PixelFormat::Rgba8, data: data.into(), sequence, time_s }
}

#[test]
fn a_letterbox_maps_model_points_back_to_the_source_frame() {
    // A 1280x720 frame in a 192x192 input: scaled by 0.15, padded top and bottom.
    let lb = Letterbox::new((1280, 720), (192, 192));
    let (x, y) = lb.to_source(0.5, 0.5);
    assert!((x - 0.5).abs() < 1e-4 && (y - 0.5).abs() < 1e-4, "the centre stays the centre");
    let (_, top) = lb.to_source(0.5, (192.0 - 108.0) / 2.0 / 192.0);
    assert!(top.abs() < 1e-3, "the top of the content is the top of the frame: {top}");
    let (_, in_padding) = lb.to_source(0.5, 0.0);
    assert!(in_padding < 0.0, "padding is outside the frame");
}

#[test]
fn letterboxing_pads_black_and_resamples_the_picture() {
    let frame = solid_frame(64, 32, [200, 100, 50, 255], 0, 0.0);
    let lb = Letterbox::new((64, 32), (32, 32));
    let rgb = letterbox_rgb(&frame, &lb);
    assert_eq!(rgb.len(), 32 * 32 * 3);
    let pixel = |x: usize, y: usize| &rgb[(y * 32 + x) * 3..(y * 32 + x) * 3 + 3];
    assert_eq!(pixel(16, 0), &[0, 0, 0], "padding is black");
    assert_eq!(pixel(16, 16), &[200, 100, 50], "the picture keeps its colour");
    let bgra = VideoFrame { format: PixelFormat::Bgra8, ..solid_frame(64, 32, [50, 100, 200, 255], 0, 0.0) };
    assert_eq!(&letterbox_rgb(&bgra, &lb)[(16 * 32 + 16) * 3..][..3], &[200, 100, 50], "BGRA is swizzled");
}

#[test]
fn decoding_keypoints_gaps_the_unsure_and_maps_to_source_fractions() {
    let lb = Letterbox::new((100, 100), (100, 100));
    let mut output = vec![0.0f32; 51];
    output[..3].copy_from_slice(&[0.25, 0.75, 0.9]); // nose: y, x, score
    output[3..6].copy_from_slice(&[0.5, 0.5, 0.05]); // an unsure eye
    let values = decode_single_person(&output, &lb, 0.25).expect("decoded");
    assert_eq!(values.len(), COCO17.len() * 3);
    assert_eq!(&values[..3], &[0.75, 0.25, 0.9], "x, y, score in image fractions");
    assert!(values[3..6].iter().all(|v| v.is_nan()), "below the threshold is a gap, not a guess");
    assert!(decode_single_person(&[0.0; 10], &lb, 0.25).is_none(), "a short output is refused");
    let rig = coco17_rig();
    rig.validate().expect("valid");
    assert_eq!(rig.columns(), values.len());
    assert!(COCO17_EDGES.iter().all(|(a, b)| *a < 17 && *b < 17));
}

struct FakeEstimator;

impl PoseEstimator for FakeEstimator {
    fn rig(&self) -> mocap_format::RigProfile {
        coco17_rig()
    }

    fn name(&self) -> &str {
        "fake"
    }

    fn estimate(&mut self, frame: &VideoFrame) -> Option<Vec<f32>> {
        // Nobody in view on odd frames.
        (frame.sequence % 2 == 0).then(|| vec![0.5; 51])
    }
}

#[test]
fn a_tracker_runs_an_estimator_on_a_cameras_frames_with_their_capture_times() {
    let camera = VideoMailbox::new();
    let bus = PoseBus::new(64);
    let sink = SourceSink { poses: bus.clone(), video: VideoMailbox::new(), clock: SessionClock::start() };
    let factory: EstimatorFactory = std::sync::Arc::new(|| Ok(Box::new(FakeEstimator) as Box<dyn PoseEstimator>));
    let mut tracker = EstimatorSource::new("camera:0", "Cam", camera.clone(), coco17_rig(), factory);
    assert_eq!(tracker.descriptor().id, "pose:camera:0");
    assert_eq!(EstimatorSource::camera_of("pose:camera:0"), Some("camera:0"));
    assert_eq!(tracker.descriptor().kind, SourceKind::Estimator);
    tracker.start(sink).expect("start");
    let mut frames = Vec::new();
    for sequence in 0..6u64 {
        camera.put(solid_frame(4, 4, [0, 0, 0, 255], sequence, 10.0 + sequence as f64));
        std::thread::sleep(Duration::from_millis(30));
        bus.drain(&mut frames);
    }
    tracker.stop();
    assert!(!frames.is_empty(), "the tracker delivered poses");
    assert!(frames.iter().all(|f| f.time_s >= 10.0), "poses carry the camera frame's time, not the tracker's");
}

#[test]
fn a_tracker_whose_model_does_not_load_reports_why_and_can_start_again() {
    let camera = VideoMailbox::new();
    let sink = SourceSink { poses: PoseBus::new(8), video: VideoMailbox::new(), clock: SessionClock::start() };
    let factory: EstimatorFactory = std::sync::Arc::new(|| Err("no model at /nowhere/pose.onnx".to_owned()));
    let mut tracker = EstimatorSource::new("camera:1", "Cam", camera, coco17_rig(), factory);
    tracker.start(sink.clone()).expect("starting is asynchronous");
    let mut state = tracker.status().state;
    for _ in 0..100 {
        if matches!(state, SourceState::Failed(_)) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
        state = tracker.status().state;
    }
    assert_eq!(state, SourceState::Failed("no model at /nowhere/pose.onnx".to_owned()));
    tracker.stop();
    assert!(tracker.start(sink).is_ok(), "stopping lets it start again");
}

#[cfg(feature = "pose")]
#[test]
fn the_bundled_model_loads_and_finds_keypoints_in_a_frame() {
    let Some(loaded) = OnnxPoseEstimator::bundled() else {
        eprintln!("no bundled pose model (offline build): skipped");
        return;
    };
    let mut estimator = loaded.expect("the bundled MoveNet loads");
    assert_eq!(estimator.rig().columns(), 51);
    // A gray frame has no person; the model still runs end to end and every
    // value is either a gap or an in-range score.
    let values = estimator.estimate(&solid_frame(640, 480, [120, 120, 120, 255], 0, 0.0)).expect("an output");
    assert_eq!(values.len(), 51);
    assert!(values.chunks_exact(3).all(|k| k[0].is_nan() || (0.0..=1.0).contains(&k[2])));
}
