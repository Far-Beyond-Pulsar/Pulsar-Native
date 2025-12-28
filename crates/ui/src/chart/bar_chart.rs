use std::rc::Rc;

use gpui::{px, App, Bounds, Hsla, Pixels, SharedString, TextAlign, Window};
use gpui_component_macros::IntoPlot;
use num_traits::{Num, ToPrimitive, FromPrimitive};

use crate::{
    plot::{
        label::Text,
        scale::{Scale, ScaleBand, ScaleLinear, Sealed},
        shape::Bar,
        Axis, AxisText, Grid, Plot, AXIS_GAP,
    },
    ActiveTheme, PixelsExt,
};

#[derive(IntoPlot)]
pub struct BarChart<T, X, Y>
where
    T: 'static,
    X: PartialEq + Into<SharedString> + 'static,
    Y: Copy + PartialOrd + Num + ToPrimitive + FromPrimitive + Sealed + 'static,
{
    data: Vec<T>,
    x: Option<Rc<dyn Fn(&T) -> X>>,
    y: Option<Rc<dyn Fn(&T) -> Y>>,
    fill: Option<Rc<dyn Fn(&T) -> Hsla>>,
    tick_margin: usize,
    label: Option<Rc<dyn Fn(&T) -> SharedString>>,
    min_y_range: Option<f64>,
    max_y_range: Option<f64>,
    reference_lines: Vec<(f64, Hsla, SharedString)>,
}

impl<T, X, Y> BarChart<T, X, Y>
where
    X: PartialEq + Into<SharedString> + 'static,
    Y: Copy + PartialOrd + Num + ToPrimitive + FromPrimitive + Sealed + 'static,
{
    pub fn new<I>(data: I) -> Self
    where
        I: IntoIterator<Item = T>,
    {
        Self {
            data: data.into_iter().collect(),
            x: None,
            y: None,
            fill: None,
            tick_margin: 1,
            label: None,
            min_y_range: None,
            max_y_range: None,
            reference_lines: vec![],
        }
    }

    pub fn x(mut self, x: impl Fn(&T) -> X + 'static) -> Self {
        self.x = Some(Rc::new(x));
        self
    }

    pub fn y(mut self, y: impl Fn(&T) -> Y + 'static) -> Self {
        self.y = Some(Rc::new(y));
        self
    }

    pub fn fill<H>(mut self, fill: impl Fn(&T) -> H + 'static) -> Self
    where
        H: Into<Hsla> + 'static,
    {
        self.fill = Some(Rc::new(move |t| fill(t).into()));
        self
    }

    pub fn tick_margin(mut self, tick_margin: usize) -> Self {
        self.tick_margin = tick_margin;
        self
    }

    pub fn label<S>(mut self, label: impl Fn(&T) -> S + 'static) -> Self
    where
        S: Into<SharedString> + 'static,
    {
        self.label = Some(Rc::new(move |t| label(t).into()));
        self
    }

    pub fn min_y_range(mut self, min: f64) -> Self {
        self.min_y_range = Some(min);
        self
    }

    pub fn max_y_range(mut self, max: f64) -> Self {
        self.max_y_range = Some(max);
        self
    }

    pub fn reference_line(mut self, y_value: f64, stroke: impl Into<Hsla>, label: impl Into<SharedString>) -> Self {
        self.reference_lines.push((y_value, stroke.into(), label.into()));
        self
    }
}

impl<T, X, Y> Plot for BarChart<T, X, Y>
where
    X: PartialEq + Into<SharedString> + 'static,
    Y: Copy + PartialOrd + Num + ToPrimitive + FromPrimitive + Sealed + 'static,
{
    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let (Some(x_fn), Some(y_fn)) = (self.x.as_ref(), self.y.as_ref()) else {
            return;
        };

        let width = bounds.size.width.as_f32();
        let height = bounds.size.height.as_f32() - AXIS_GAP;

        // X scale
        let x = ScaleBand::new(self.data.iter().map(|v| x_fn(v)).collect(), vec![0., width])
            .padding_inner(0.4)
            .padding_outer(0.2);
        let band_width = x.band_width();

        // Y scale with min/max range enforcement
        let domain_vals: Vec<_> = self.data
            .iter()
            .map(|v| y_fn(v))
            .chain(Some(Y::zero()))
            .collect();
        
        let mut domain = domain_vals.clone();
        
        // Enforce minimum Y range if specified by ensuring domain spans at least min_y_range
        if let Some(min_range) = self.min_y_range {
            let mut min_f = f64::INFINITY;
            let mut max_f = f64::NEG_INFINITY;
            
            // Calculate actual data range
            for &val in domain.iter() {
                if let Some(f) = val.to_f64() {
                    min_f = min_f.min(f);
                    max_f = max_f.max(f);
                }
            }
            
            // If range is smaller than minimum, extend it
            if !min_f.is_infinite() && !max_f.is_infinite() {
                let range = max_f - min_f;
                if range < min_range && range.is_finite() {
                    // Calculate the target maximum value
                    let target_max = min_f + min_range;
                    
                    // Ensure target_max is in the domain
                    if let Some(target_y) = Y::from_f64(target_max) {
                        // Check if we already have this value
                        let already_present = domain.iter().any(|&v| {
                            v.to_f64().map_or(false, |f| (f - target_max).abs() < 0.0001)
                        });
                        if !already_present {
                            domain.push(target_y);
                        }
                    }
                }
            }
        }
        
        // Enforce maximum Y range if specified
        if let Some(max_range) = self.max_y_range {
            if let Some(capped_max) = Y::from_f64(max_range) {
                domain.push(capped_max);
            }
        }
        
        let y = ScaleLinear::new(domain, vec![height, 10.]);

        // Draw X axis
        let x_label = self.data.iter().enumerate().filter_map(|(i, d)| {
            if (i + 1) % self.tick_margin == 0 {
                x.tick(&x_fn(d)).map(|x_tick| {
                    AxisText::new(
                        x_fn(d).into(),
                        x_tick + band_width / 2.,
                        cx.theme().muted_foreground,
                    )
                    .align(TextAlign::Center)
                })
            } else {
                None
            }
        });

        Axis::new()
            .x(height)
            .x_label(x_label)
            .stroke(cx.theme().border)
            .paint(&bounds, window, cx);

        // Draw grid
        Grid::new()
            .y((0..=3).map(|i| height * i as f32 / 4.0).collect())
            .stroke(cx.theme().border)
            .dash_array(&[px(4.), px(2.)])
            .paint(&bounds, window);

        // Draw bars
        let x_fn = x_fn.clone();
        let y_fn = y_fn.clone();
        let default_fill = cx.theme().chart_2;
        let fill = self.fill.clone();
        let label_color = cx.theme().foreground;
        let y_clone = y.clone();
        let mut bar = Bar::new()
            .data(&self.data)
            .band_width(band_width)
            .x(move |d| x.tick(&x_fn(d)))
            .y0(height)
            .y1(move |d| y_clone.tick(&y_fn(d)))
            .fill(move |d| fill.as_ref().map(|f| f(d)).unwrap_or(default_fill));

        if let Some(label) = self.label.as_ref() {
            let label = label.clone();
            bar = bar.label(move |d, p| vec![Text::new(label(d), p, label_color)]);
        }

        bar.paint(&bounds, window, cx);

        // Draw reference lines
        for (y_value, color, _label) in &self.reference_lines {
            if let Some(y_tick) = y.tick(&Y::from_f64(*y_value).unwrap_or_else(Y::zero)) {
                Grid::new()
                    .y(vec![y_tick])
                    .stroke(*color)
                    .dash_array(&[px(2.), px(2.)])
                    .paint(&bounds, window);
            }
        }
    }
}
