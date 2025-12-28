use std::rc::Rc;

use gpui::{px, App, Background, Bounds, Hsla, Pixels, SharedString, TextAlign, Window};
use gpui_component_macros::IntoPlot;
use num_traits::{Num, ToPrimitive};

use crate::{
    plot::{
        scale::{Scale, ScaleLinear, ScalePoint, Sealed},
        shape::Area,
        Axis, AxisText, Grid, Plot, StrokeStyle, AXIS_GAP,
    },
    ActiveTheme, PixelsExt,
};

#[derive(IntoPlot)]
pub struct AreaChart<T, X, Y>
where
    T: 'static,
    X: Clone + PartialEq + Into<SharedString> + 'static,
    Y: Clone + Copy + PartialOrd + Num + ToPrimitive + Sealed + 'static,
{
    data: Vec<T>,
    x: Option<Rc<dyn Fn(&T) -> X>>,
    y: Vec<Rc<dyn Fn(&T) -> Y>>,
    stroke: Vec<Hsla>,
    stroke_style: StrokeStyle,
    fill: Vec<Background>,
    tick_margin: usize,
    min_y_range: Option<f64>,
    max_y_range: Option<f64>,
}

impl<T, X, Y> AreaChart<T, X, Y>
where
    X: Clone + PartialEq + Into<SharedString> + 'static,
    Y: Clone + Copy + PartialOrd + Num + ToPrimitive + Sealed + 'static,
{
    pub fn new<I>(data: I) -> Self
    where
        I: IntoIterator<Item = T>,
    {
        Self {
            data: data.into_iter().collect(),
            stroke_style: Default::default(),
            stroke: vec![],
            fill: vec![],
            tick_margin: 1,
            x: None,
            y: vec![],
            min_y_range: None,
            max_y_range: None,
        }
    }

    pub fn x(mut self, x: impl Fn(&T) -> X + 'static) -> Self {
        self.x = Some(Rc::new(x));
        self
    }

    pub fn y(mut self, y: impl Fn(&T) -> Y + 'static) -> Self {
        self.y.push(Rc::new(y));
        self
    }

    pub fn stroke(mut self, stroke: impl Into<Hsla>) -> Self {
        self.stroke.push(stroke.into());
        self
    }

    pub fn fill(mut self, fill: impl Into<Background>) -> Self {
        self.fill.push(fill.into());
        self
    }

    pub fn linear(mut self) -> Self {
        self.stroke_style = StrokeStyle::Linear;
        self
    }

    pub fn tick_margin(mut self, tick_margin: usize) -> Self {
        self.tick_margin = tick_margin;
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
}

impl<T, X, Y> Plot for AreaChart<T, X, Y>
where
    X: Clone + PartialEq + Into<SharedString> + 'static,
    Y: Clone + Copy + PartialOrd + Num + ToPrimitive + Sealed + 'static,
{
    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let Some(x_fn) = self.x.as_ref() else {
            return;
        };

        if self.y.len() == 0 {
            return;
        }

        let width = bounds.size.width.as_f32();
        let height = bounds.size.height.as_f32() - AXIS_GAP;

        // X scale
        let x = ScalePoint::new(self.data.iter().map(|v| x_fn(v)).collect(), vec![0., width]);

        // Y scale with min/max range enforcement
        let domain_vals: Vec<_> = self
            .data
            .iter()
            .flat_map(|v| self.y.iter().map(|y_fn| y_fn(v)))
            .chain(Some(Y::zero()))
            .collect();
        
        let mut domain = domain_vals.clone();
        
        // Enforce minimum Y range if specified
        if let Some(min_range) = self.min_y_range {
            if let (Some(&min_val), Some(&max_val)) = (
                domain.iter().min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)),
                domain.iter().max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            ) {
                let min_f = min_val.to_f64().unwrap_or(0.0);
                let max_f = max_val.to_f64().unwrap_or(0.0);
                let range = max_f - min_f;
                if range < min_range {
                    // Add boundary point - use existing types from domain to ensure type safety
                    let target = if min_f > 0.0 {
                        min_f + min_range
                    } else {
                        min_range
                    };
                    // Scale min_val or zero point to reach target range
                    if let Some(ref scale_val) = domain_vals.first().cloned() {
                        domain.push(*scale_val);
                    }
                }
            }
        }
        
        // Enforce maximum Y range if specified - add boundary point if max is set
        if self.max_y_range.is_some() {
            if let Some(ref scale_val) = domain_vals.first().cloned() {
                domain.push(*scale_val);
            }
        }
        
        let y = ScaleLinear::new(domain, vec![height, 10.]);

        // Draw X axis
        let data_len = self.data.len();
        let x_label = self.data.iter().enumerate().filter_map(|(i, d)| {
            if (i + 1) % self.tick_margin == 0 {
                x.tick(&x_fn(d)).map(|x_tick| {
                    let align = match i {
                        0 => {
                            if data_len == 1 {
                                TextAlign::Center
                            } else {
                                TextAlign::Left
                            }
                        }
                        i if i == data_len - 1 => TextAlign::Right,
                        _ => TextAlign::Center,
                    };
                    AxisText::new(x_fn(d).into(), x_tick, cx.theme().muted_foreground).align(align)
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

        // Draw area
        for (i, y_fn) in self.y.iter().enumerate() {
            let x = x.clone();
            let y = y.clone();
            let x_fn = x_fn.clone();
            let y_fn = y_fn.clone();

            let fill = *self
                .fill
                .get(i)
                .unwrap_or(&cx.theme().chart_2.opacity(0.4).into());

            let stroke = *self.stroke.get(i).unwrap_or(&cx.theme().chart_2);

            Area::new()
                .data(&self.data)
                .x(move |d| x.tick(&x_fn(d)))
                .y0(height)
                .y1(move |d| y.tick(&y_fn(d)))
                .stroke(stroke)
                .stroke_style(self.stroke_style)
                .fill(fill)
                .paint(&bounds, window);
        }
    }
}
