use helio_lens_optics::*;

const TOL: f64 = 1e-9;

fn plano(glass: Glass, thickness: f64, rest: Vec<Surface>, ap: f64) -> Prescription {
    let mut front = Surface::spherical(0.0, thickness, glass, ap);
    front.stop = true;
    let mut surfaces = vec![front];
    surfaces.extend(rest);
    Prescription {
        name: "test".into(),
        source: String::new(),
        focal_length_mm: 50.0,
        f_number: 5.0,
        sensor_half_diagonal_mm: 10.0,
        surfaces,
    }
}

// ---------------------------------------------------------------- glass

#[test]
fn nbk7_matches_datasheet() {
    let g = catalog_glass("n-bk7").unwrap();
    assert!((g.nd().unwrap() - 1.5168).abs() < 1e-4);
    assert!((g.abbe().unwrap() - 64.17).abs() < 0.1);
    assert!((catalog_glass("F2").unwrap().nd().unwrap() - 1.62004).abs() < 1e-4);
    assert!(catalog_glass("nonexistent").is_none());
    assert_eq!(catalog_glass("air").unwrap().index_at(500.0), Some(1.0));
}

#[test]
fn abbe_model_reproduces_nd_and_vd() {
    let g = Glass::Abbe { nd: 1.7, vd: 35.0 };
    assert!((g.nd().unwrap() - 1.7).abs() < 1e-12);
    assert!((g.abbe().unwrap() - 35.0).abs() < 1e-9);
    // Normal dispersion: blue bends more.
    assert!(g.index_at(450.0).unwrap() > g.index_at(650.0).unwrap());
}

// -------------------------------------------------------------- paraxial

#[test]
fn singlet_efl_matches_lensmaker() {
    let (n, r1, r2, t) = (1.5168, 50.0, -80.0, 6.0);
    let rx = plano(Glass::Index(n), t, vec![], 8.0);
    let mut rx = rx;
    rx.surfaces[0].radius = r1;
    rx.surfaces
        .push(Surface::spherical(r2, 40.0, Glass::Air, 8.0));
    rx.surfaces[0].thickness = t;
    let p = Paraxial::new(&rx, 550.0).unwrap();
    let inv_f = (n - 1.0) * (1.0 / r1 - 1.0 / r2 + (n - 1.0) * t / (n * r1 * r2));
    assert!(
        (p.efl - 1.0 / inv_f).abs() < 1e-9,
        "{} vs {}",
        p.efl,
        1.0 / inv_f
    );
    // BFL from the principal plane: f (1 - (n-1) t / (n R1)).
    let bfl = p.efl * (1.0 - (n - 1.0) * t / (n * r1));
    assert!((p.bfl - bfl).abs() < 1e-9);
}

#[test]
fn flat_plate_shifts_focus_by_t_one_minus_inv_n() {
    let n = 1.5;
    let t = 6.0;
    let mut lens = Prescription::ideal_thin_lens(50.0, 5.0, 10.0);
    lens.surfaces[0].thickness = 10.0;
    lens.surfaces
        .push(Surface::spherical(0.0, t, Glass::Index(n), 8.0));
    lens.surfaces
        .push(Surface::spherical(0.0, 0.0, Glass::Air, 8.0));
    let p = Paraxial::new(&lens, 550.0).unwrap();
    assert!((p.focus_z - (50.0 + t * (1.0 - 1.0 / n))).abs() < TOL);

    // Real near-axis ray agrees.
    let last = lens.surfaces.len() - 1;
    lens.surfaces[last].thickness = p.focus_z - lens.vertex_z()[last];
    let tr = Tracer::new(&lens, 550.0).unwrap();
    for h in [1e-3, 2e-3] {
        let ray = tr
            .aim(Field::Infinity { tan: [0.0, 0.0] }, [h, 0.0])
            .unwrap();
        let hit = tr.trace(ray).unwrap();
        assert!(hit.origin[0].abs() < 1e-5 * h.max(1.0), "{:?}", hit);
    }
}

#[test]
fn finite_object_thin_lens_conjugates() {
    let rx = Prescription::ideal_thin_lens(50.0, 2.0, 21.6);
    let p = Paraxial::new(&rx, 550.0).unwrap();
    let v = p.image_distance(1000.0).unwrap();
    assert!((v - 1.0 / (1.0 / 50.0 - 1.0 / 1000.0)).abs() < TOL);
    let f = rx.focused(1000.0, FocusMode::Sensor, 550.0).unwrap();
    assert!((f.sensor_z() - v).abs() < TOL);
}

#[test]
fn group_focus_refocuses_real_lens() {
    let rx = examples::double_gauss();
    let d = 1000.0;
    let g = rx
        .focused(
            d,
            FocusMode::Group {
                first: 6,
                last: rx.surfaces.len() - 1,
            },
            D_LINE_NM,
        )
        .unwrap();
    let p = Paraxial::new(&g, D_LINE_NM).unwrap();
    let z = p.image_z(&g, d).unwrap();
    assert!((z - g.sensor_z()).abs() < 1e-8, "{z} vs {}", g.sensor_z());
    // Sensor and front are fixed.
    assert!((g.sensor_z() - rx.sensor_z()).abs() < 1e-9);
    // Unit focusing moves the sensor back.
    let s = rx.focused(d, FocusMode::Sensor, D_LINE_NM).unwrap();
    assert!(s.sensor_z() > rx.sensor_z());
    let ps = Paraxial::new(&s, D_LINE_NM).unwrap();
    assert!((ps.image_z(&s, d).unwrap() - s.sensor_z()).abs() < 1e-8);
}

#[test]
fn double_gauss_first_order() {
    let rx = examples::double_gauss();
    let p = Paraxial::new(&rx, D_LINE_NM).unwrap();
    // 100 mm design scaled to 50 mm.
    assert!((p.efl - 50.0).abs() < 0.05, "efl {}", p.efl);
    assert!((p.f_number - 2.0).abs() < 0.01, "f/{}", p.f_number);
    assert!((p.sensor_z - p.focus_z).abs() < 1e-3);
    assert!(p.bfl > 5.0);
    // Pupils are real-sized and nearby.
    assert!((p.entrance_pupil_radius - 12.5).abs() < 0.1);
    assert!(p.exit_pupil_radius > 5.0);
}

#[test]
fn cooke_first_order() {
    let rx = examples::cooke_triplet();
    let p = Paraxial::new(&rx, D_LINE_NM).unwrap();
    assert!((p.efl - 50.0).abs() < 0.05, "efl {}", p.efl);
    assert!((p.f_number - 5.0).abs() < 0.01);
}

// ------------------------------------------------------------------ trace

#[test]
fn snell_refraction() {
    let th_i = 30f64.to_radians();
    let d = [th_i.sin(), 0.0, th_i.cos()];
    let t = refract(d, [0.0, 0.0, 1.0], 1.0 / 1.5).unwrap();
    assert!((t[0] - th_i.sin() / 1.5).abs() < 1e-12);
    assert!((t[0] * t[0] + t[2] * t[2] - 1.0).abs() < 1e-12);
    // Normal sign is irrelevant.
    let t2 = refract(d, [0.0, 0.0, -1.0], 1.0 / 1.5).unwrap();
    assert_eq!(t, t2);
}

#[test]
fn total_internal_reflection_threshold() {
    let crit = (1.0f64 / 1.5).asin();
    let below = crit - 0.01;
    let above = crit + 0.01;
    let dir = |a: f64| [a.sin(), 0.0, a.cos()];
    assert!(refract(dir(below), [0.0, 0.0, 1.0], 1.5).is_some());
    assert!(refract(dir(above), [0.0, 0.0, 1.0], 1.5).is_none());
}

#[test]
fn plate_leaves_direction_unchanged_and_shifts_ray() {
    let rx = plano(
        Glass::Index(1.5),
        10.0,
        vec![Surface::spherical(0.0, 10.0, Glass::Air, 50.0)],
        50.0,
    );
    let tr = Tracer::new(&rx, 550.0).unwrap();
    let a = 40f64.to_radians();
    let dir = [a.sin(), 0.0, a.cos()];
    let ray = Ray {
        origin: [0.0, 0.0, -5.0],
        dir,
    };
    let mut inside = None;
    let out = tr
        .trace_with(ray, |i, r| {
            if i == 0 {
                inside = Some(r.dir);
            }
        })
        .unwrap();
    let inside = inside.unwrap();
    assert!((inside[0] - a.sin() / 1.5).abs() < 1e-12);
    assert!((out.dir[0] - dir[0]).abs() < 1e-12 && (out.dir[2] - dir[2]).abs() < 1e-12);
}

#[test]
fn total_internal_reflection_in_trace() {
    // Convex exit surface: a ray at h = 9 hits R = 10 at 64 deg > 30 deg critical (n = 2).
    let rx = plano(
        Glass::Index(2.0),
        8.0,
        vec![Surface::spherical(-10.0, 10.0, Glass::Air, 9.9)],
        9.9,
    );
    let tr = Tracer::new(&rx, 550.0).unwrap();
    let ray = Ray {
        origin: [9.0, 0.0, -1.0],
        dir: [0.0, 0.0, 1.0],
    };
    let err = tr.trace(ray).unwrap_err();
    assert_eq!(
        err,
        RayFail {
            surface: 1,
            kind: RayFailKind::TotalInternalReflection
        }
    );
}

#[test]
fn clear_aperture_vignettes_when_clipped() {
    let rx = Prescription::ideal_thin_lens(50.0, 2.0, 21.6);
    let ray = Ray {
        origin: [13.0, 0.0, -1.0],
        dir: [0.0, 0.0, 1.0],
    };
    let tr = Tracer::new(&rx, 550.0).unwrap();
    assert_eq!(tr.trace(ray).unwrap_err().kind, RayFailKind::Aperture);
    assert!(tr.trace_unclipped(ray).is_ok());
    let inside = Ray {
        origin: [12.0, 0.0, -1.0],
        dir: [0.0, 0.0, 1.0],
    };
    assert!(tr.trace(inside).is_ok());
}

#[test]
fn sphere_intersection_matches_analytic_sag() {
    let s = Surface::spherical(30.0, 1.0, Glass::Air, 20.0);
    let h = 12.0;
    let t = s.intersect([h, 0.0, -5.0], [0.0, 0.0, 1.0]).unwrap();
    let z = -5.0 + t;
    assert!((z - (30.0 - (30.0f64 * 30.0 - h * h).sqrt())).abs() < 1e-12);
    assert!(s.intersect([40.0, 0.0, -5.0], [0.0, 0.0, 1.0]).is_none());
}

#[test]
fn asphere_normal_matches_sag_gradient() {
    let mut s = Surface::spherical(35.0, 1.0, Glass::Air, 20.0);
    s.conic = -0.6;
    s.aspheric = [1e-6, -2e-9, 3e-12, 0.0, 0.0];
    let (x, y) = (7.0, -4.0);
    let e = 1e-6;
    let sag = |x: f64, y: f64| s.sag(x * x + y * y);
    let gx = (sag(x + e, y) - sag(x - e, y)) / (2.0 * e);
    let gy = (sag(x, y + e) - sag(x, y - e)) / (2.0 * e);
    let n = s.normal(x, y);
    let l = (gx * gx + gy * gy + 1.0).sqrt();
    assert!(
        (n[0] + gx / l).abs() < 1e-7
            && (n[1] + gy / l).abs() < 1e-7
            && (n[2] - 1.0 / l).abs() < 1e-7
    );
    // Intersection lands on the surface.
    let o = [x, y, -3.0];
    let t = s.intersect(o, [0.0, 0.0, 1.0]).unwrap();
    assert!((o[2] + t - sag(x, y)).abs() < 1e-10);
}

#[test]
fn conic_lens_focuses_collimated_light_perfectly() {
    // Flat front, hyperboloidal rear with k = -n^2 (Descartes oval for glass to air).
    let n = 1.5;
    let r0 = 20.0;
    let mut rear = Surface::spherical(-r0, 30.0, Glass::Air, 9.0);
    rear.conic = -(n * n);
    let mut rx = plano(Glass::Index(n), 4.0, vec![rear], 9.0);
    let p = Paraxial::new(&rx, 550.0).unwrap();
    let last = rx.surfaces.len() - 1;
    rx.surfaces[last].thickness = p.bfl;
    let tr = Tracer::new(&rx, 550.0).unwrap();
    for h in [1.0, 4.0, 8.0] {
        let ray = tr
            .aim(Field::Infinity { tan: [0.0, 0.0] }, [h / 9.0, 0.0])
            .unwrap();
        let hit = tr.trace(ray).unwrap();
        assert!(hit.origin[0].abs() < 1e-8, "h={h}: {}", hit.origin[0]);
    }
    // The spherical equivalent is clearly aberrated.
    rx.surfaces[last].conic = 0.0;
    let tr = Tracer::new(&rx, 550.0).unwrap();
    let ray = tr
        .aim(Field::Infinity { tan: [0.0, 0.0] }, [8.0 / 9.0, 0.0])
        .unwrap();
    assert!(tr.trace(ray).unwrap().origin[0].abs() > 0.05);
}

#[test]
fn aim_hits_requested_stop_point() {
    for rx in examples::all() {
        let tr = Tracer::new(&rx, 550.0).unwrap();
        let rs = tr.stop_radius();
        for tan in [0.0, 0.2] {
            let f = Field::meridional(tan, None);
            for uv in [[0.0, 0.0], [0.6, 0.0], [-0.3, 0.5]] {
                let ray = tr.aim(f, uv).unwrap();
                let h = tr.stop_hit(ray).unwrap();
                assert!(
                    (h[0] - uv[0] * rs).abs() < 1e-6 * rs && (h[1] - uv[1] * rs).abs() < 1e-6 * rs,
                    "{}",
                    rx.name
                );
            }
        }
    }
}

#[test]
fn trace_is_deterministic() {
    let rx = examples::double_gauss();
    let tr = Tracer::new(&rx, 550.0).unwrap();
    let f = Field::meridional(0.25, None);
    let a = tr.trace(tr.aim(f, [0.3, -0.2]).unwrap()).unwrap();
    let b = tr.trace(tr.aim(f, [0.3, -0.2]).unwrap()).unwrap();
    assert_eq!(a, b);
}

#[test]
fn real_lens_focuses_near_paraxial_focus() {
    for rx in [examples::double_gauss(), examples::cooke_triplet()] {
        let tr = Tracer::new(&rx, D_LINE_NM).unwrap();
        let ray = tr
            .aim(Field::Infinity { tan: [0.0, 0.0] }, [1e-3, 0.0])
            .unwrap();
        let hit = tr.trace(ray).unwrap();
        assert!(hit.origin[0].abs() < 1e-4, "{}: {}", rx.name, hit.origin[0]);
    }
}

// ----------------------------------------------------------------- spectral

#[test]
fn spectral_weights_reconstruct_white() {
    for n in [1, 3, 7, 16] {
        let s = SpectralSampling::uniform(n);
        assert_eq!(s.len(), n);
        for sum in s.channel_sums() {
            assert!((sum - 1.0).abs() < 1e-6, "n={n}: {sum}");
        }
        let white = s.reconstruct(&vec![[1.0, 1.0, 1.0]; n]);
        for c in white {
            assert!((c - 1.0).abs() < 1e-6);
        }
    }
    assert_eq!(
        SpectralSampling::uniform(3).wavelengths_nm,
        vec![450.0, 550.0, 650.0]
    );
    // Custom wavelengths too.
    let s = SpectralSampling::with_wavelengths(vec![470.0, 520.0, 580.0, 640.0]);
    for sum in s.channel_sums() {
        assert!((sum - 1.0).abs() < 1e-6);
    }
}

// -------------------------------------------------------------------- serde

#[test]
fn examples_round_trip() {
    for rx in examples::all() {
        rx.validate().unwrap();
        let back = Prescription::from_json(&rx.to_json().unwrap()).unwrap();
        assert_eq!(rx, back);
    }
    // Bare data files parse identically to the embedded constants.
    let on_disk = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/data/double_gauss_50mm_f2.json"
    ))
    .unwrap();
    assert_eq!(
        Prescription::from_json(&on_disk).unwrap(),
        examples::double_gauss()
    );
}

#[test]
fn invalid_prescriptions_are_rejected() {
    let mut rx = examples::cooke_triplet();
    rx.surfaces[0].glass = Glass::Catalog("UNOBTAINIUM".into());
    assert!(matches!(
        rx.validate(),
        Err(OpticsError::UnknownGlass { surface: 0 })
    ));
    let mut rx = examples::cooke_triplet();
    rx.surfaces[4].stop = false;
    assert!(matches!(rx.validate(), Err(OpticsError::StopCount(0))));
    assert!(Prescription::from_json("{").is_err());
}

// -------------------------------------------------------------------- bakes

#[test]
fn thin_lens_has_zero_distortion_and_cos4_falloff() {
    let rx = examples::thin_lens();
    let cfg = BakeConfig {
        field_samples: 11,
        pupil_samples: 12,
        ..BakeConfig::coarse()
    };
    let lut = bake_ray_map(&rx, &cfg).unwrap();
    assert_eq!(lut.valid_field, 1.0);
    for w in 0..lut.n_wave {
        for i in 0..lut.n_field {
            assert!(
                lut.distortion(w, i).abs() < 1e-6,
                "w{w} i{i}: {}",
                lut.distortion(w, i)
            );
            let y = rx.sensor_half_diagonal_mm * lut.field_r(i) as f64;
            assert!((lut.tan_theta(w, i) as f64 - y / 50.0).abs() < 1e-6);
        }
    }
    let v = bake_vignetting(&rx, &cfg).unwrap();
    for j in 0..v.n_fnum {
        for i in 0..v.n_field {
            let theta =
                (rx.sensor_half_diagonal_mm * i as f64 / (v.n_field - 1) as f64 / 50.0).atan();
            assert!((v.at(j, i) as f64 - theta.cos().powi(4)).abs() < 1e-5);
        }
    }
}

#[test]
fn vignetting_is_one_on_axis_and_monotone() {
    let cfg = BakeConfig {
        field_samples: 13,
        pupil_samples: 16,
        ..BakeConfig::coarse()
    };
    for rx in examples::all() {
        let v = bake_vignetting(&rx, &cfg).unwrap();
        assert!(v.n_fnum >= 2, "{}: expected stop-down rows", rx.name);
        assert!((v.f_numbers[0] as f64 - rx.f_number).abs() < 1e-6);
        for j in 0..v.n_fnum {
            assert_eq!(v.at(j, 0), 1.0, "{} f/{}", rx.name, v.f_numbers[j]);
            for i in 1..v.n_field {
                assert!(
                    v.at(j, i) <= v.at(j, i - 1),
                    "{} f/{} i{i}",
                    rx.name,
                    v.f_numbers[j]
                );
                assert!((0.0..=1.0).contains(&v.at(j, i)));
            }
        }
    }
}

#[test]
fn real_lens_vignettes_wide_open_and_less_stopped_down() {
    let rx = examples::double_gauss();
    let cfg = BakeConfig {
        field_samples: 9,
        pupil_samples: 20,
        f_numbers: vec![2.0, 8.0],
        ..BakeConfig::coarse()
    };
    let v = bake_vignetting(&rx, &cfg).unwrap();
    let corner = v.n_field - 1;
    assert!(v.at(0, corner) < 0.7, "open corner {}", v.at(0, corner));
    assert!(
        v.at(1, corner) > v.at(0, corner) + 0.05,
        "stopped corner {}",
        v.at(1, corner)
    );
}

#[test]
fn real_lens_ray_map_is_physical() {
    let rx = examples::double_gauss();
    let cfg = BakeConfig {
        spectral: SpectralSampling::with_wavelengths(vec![F_LINE_NM, D_LINE_NM, C_LINE_NM]),
        field_samples: 9,
        ..BakeConfig::coarse()
    };
    let lut = bake_ray_map(&rx, &cfg).unwrap();
    assert!(lut.valid_field > 0.99);
    let n = lut.n_field;
    // Reference wavelength: zero distortion on axis; magnitude bounded.
    assert!(lut.distortion(1, 0).abs() < 1e-9);
    for i in 0..n {
        assert!(lut.distortion(1, i).abs() < 0.1);
    }
    // tan(theta) increases with sensor radius.
    for i in 1..n {
        assert!(lut.tan_theta(1, i) > lut.tan_theta(1, i - 1));
    }
    // Off-axis wavelengths separate (lateral colour) but stay within a few percent.
    assert!((lut.distortion(0, 0) - lut.distortion(2, 0)).abs() < 1e-2);
    let spread = (lut.tan_theta(0, n - 1) - lut.tan_theta(2, n - 1)).abs();
    assert!(
        spread > 0.0 && spread < 0.02 * lut.tan_theta(1, n - 1),
        "{spread}"
    );
}

#[test]
fn chromatic_focus_shift_has_normal_sign() {
    let rx = examples::cooke_triplet();
    let cfg = BakeConfig {
        spectral: SpectralSampling::with_wavelengths(vec![F_LINE_NM, D_LINE_NM, C_LINE_NM]),
        ..BakeConfig::coarse()
    };
    let f = bake_focus(&rx, &cfg).unwrap();
    assert!(f.axial_paraxial_mm[1].abs() < 1e-3);
    assert!(f.axial_paraxial_mm[0] < f.axial_paraxial_mm[1]);
    assert!(f.axial_paraxial_mm[2] > f.axial_paraxial_mm[1]);
    assert!(f.axial_real_mm.iter().all(|v| v.is_finite()));
    assert_eq!(f.best_mm.len(), f.n_field);
}

#[test]
fn ideal_lens_has_no_aberration_and_defocus_blur_scales() {
    let rx = examples::thin_lens();
    let cfg = BakeConfig {
        defocus_mm: vec![-0.2, 0.0, 0.2],
        ..BakeConfig::coarse()
    };
    let psf = bake_psf(&rx, &cfg).unwrap();
    assert_eq!(psf.stride, PSF_FIXED_FIELDS + psf.n_profile);
    for i in 0..psf.n_field {
        assert!(psf.record(i, 1)[0] < 1e-9, "in-focus spot");
        let (a, b) = (psf.record(i, 0), psf.record(i, 2));
        assert!((a[0] - b[0]).abs() < 1e-6 * a[0].max(1.0));
        // Geometric blur disc: edge radius <= |dz| / (2 N) = 0.05 (grid stops short of the rim).
        assert!(
            a[4] as f64 > 0.9 * 0.05 && (a[4] as f64) < 0.05 * 1.001,
            "edge {}",
            a[4]
        );
        assert_eq!(*a.last().unwrap(), 1.0);
    }
    // Uniform disc: encircled energy ~ (r/R)^2.
    let rec = psf.record(0, 0);
    let half = rec[PSF_FIXED_FIELDS + psf.n_profile / 2 - 1];
    assert!((half - 0.25).abs() < 0.08, "{half}");
}

#[test]
fn footprints_show_cats_eye() {
    let rx = examples::double_gauss();
    let cfg = BakeConfig {
        footprint_fields: vec![0.0, 1.0],
        footprint_resolution: 24,
        footprint_supersample: 2,
        ..BakeConfig::coarse()
    };
    let fp = bake_footprints(&rx, &cfg).unwrap();
    let centre_area: f32 = fp.mask(0).iter().sum();
    let ideal = std::f32::consts::PI * 12.0 * 12.0;
    assert!(
        (centre_area - ideal).abs() < 0.06 * ideal,
        "{centre_area} vs {ideal}"
    );
    assert!(
        fp.relative_area(1) < 0.9,
        "corner area {}",
        fp.relative_area(1)
    );
    assert!(fp.relative_area(1) > 0.1);
    assert!(fp.mask(1).iter().all(|v| (0.0..=1.0).contains(v)));
}

#[test]
fn full_bake_layouts_are_consistent_and_finite() {
    for rx in examples::all() {
        let cfg = BakeConfig {
            pupil_samples: 10,
            ..BakeConfig::coarse()
        };
        let b = LensBake::bake(&rx, &cfg).unwrap();
        assert_eq!(
            b.ray_map.data.len(),
            2 * b.ray_map.n_wave * b.ray_map.n_field
        );
        assert_eq!(
            b.vignetting.data.len(),
            b.vignetting.n_fnum * b.vignetting.n_field
        );
        assert_eq!(
            b.footprints.data.len(),
            b.footprints.n_fields * b.footprints.resolution.pow(2)
        );
        assert_eq!(
            b.psf.data.len(),
            b.psf.n_field * b.psf.n_defocus * b.psf.stride
        );
        assert_eq!(b.ray_map.as_bytes().len(), b.ray_map.data.len() * 4);
        for d in [
            &b.ray_map.data,
            &b.vignetting.data,
            &b.footprints.data,
            &b.psf.data,
            &b.focus.best_mm,
        ] {
            assert!(d.iter().all(|v| v.is_finite()), "{}", rx.name);
        }
        let again = LensBake::bake(&rx, &cfg).unwrap();
        assert_eq!(b.ray_map, again.ray_map);
        assert_eq!(b.vignetting, again.vignetting);
    }
}

#[test]
fn near_focus_bake_uses_refocused_lens() {
    let rx = examples::double_gauss();
    let cfg = BakeConfig {
        focus_distance_mm: Some(500.0),
        pupil_samples: 10,
        field_samples: 5,
        ..BakeConfig::coarse()
    };
    let b = LensBake::bake(&rx, &cfg).unwrap();
    assert!(b.ray_map.valid_field > 0.9);
    assert!(b.vignetting.data.iter().all(|v| v.is_finite()));
    // On axis, in-focus spot is small even at 0.5 m.
    let rec = b.psf.record(0, 1);
    assert!(rec[0] < 0.05, "rms {}", rec[0]);
}
