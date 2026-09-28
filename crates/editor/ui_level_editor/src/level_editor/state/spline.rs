//! Spline authoring data. Curves are serialized on scene objects; this domain
//! holds only the current tool and panel preferences.

use glam::Vec3;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CurveAlgorithm { Linear, #[default] CatmullRom, Bezier, Hermite, BSpline }
impl CurveAlgorithm {
    pub const ALL: [Self; 5] = [Self::Linear, Self::CatmullRom, Self::Bezier, Self::Hermite, Self::BSpline];
    pub fn key(self) -> &'static str { match self {
        Self::Linear => "LevelEditor.SplinePanel.Linear", Self::CatmullRom => "LevelEditor.SplinePanel.CatmullRom",
        Self::Bezier => "LevelEditor.SplinePanel.Bezier", Self::Hermite => "LevelEditor.SplinePanel.Hermite",
        Self::BSpline => "LevelEditor.SplinePanel.BSpline",
    }}
    pub fn description(self) -> &'static str { match self {
        Self::Linear => "LevelEditor.SplinePanel.LinearHelp", Self::CatmullRom => "LevelEditor.SplinePanel.CatmullHelp",
        Self::Bezier => "LevelEditor.SplinePanel.BezierHelp", Self::Hermite => "LevelEditor.SplinePanel.HermiteHelp",
        Self::BSpline => "LevelEditor.SplinePanel.BSplineHelp",
    }}
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SplinePoint {
    pub position: [f32; 3],
    pub arrive: [f32; 3],
    pub leave: [f32; 3],
}
impl SplinePoint { pub fn new(position: [f32; 3]) -> Self { Self { position, arrive: [0.; 3], leave: [0.; 3] } } }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SplineData {
    pub points: Vec<SplinePoint>,
    pub algorithm: CurveAlgorithm,
    pub closed: bool,
    pub resolution: u32,
    pub tension: f32,
}
impl Default for SplineData {
    fn default() -> Self { Self { points: vec![], algorithm: CurveAlgorithm::CatmullRom, closed: false, resolution: 24, tension: 0. } }
}
impl SplineData {
    pub fn segment_count(&self) -> usize { if self.points.len() < 2 { 0 } else if self.closed { self.points.len() } else { self.points.len() - 1 } }
    fn position(&self, index: isize) -> Vec3 {
        let n = self.points.len() as isize;
        let i = if self.closed { index.rem_euclid(n) } else { index.clamp(0, n - 1) };
        Vec3::from_array(self.points[i as usize].position)
    }
    pub fn evaluate(&self, t: f32) -> [f32; 3] {
        if self.points.is_empty() { return [0.; 3]; }
        if self.points.len() == 1 { return self.points[0].position; }
        let t = t.clamp(0., 1.);
        if self.algorithm == CurveAlgorithm::BSpline && !self.closed { return self.de_boor(t).to_array(); }
        let count = self.segment_count();
        let scaled = t * count as f32;
        let i = (scaled as usize).min(count - 1);
        let u = scaled - i as f32;
        let a = self.position(i as isize);
        let b = self.position(i as isize + 1);
        let prev = self.position(i as isize - 1);
        let next = self.position(i as isize + 2);
        let u2 = u*u; let u3 = u2*u;
        let value = match self.algorithm {
            CurveAlgorithm::Linear => a.lerp(b, u),
            CurveAlgorithm::Bezier => {
                let p = a + Vec3::from_array(self.points[i].leave);
                let q = b - Vec3::from_array(self.points[(i+1)%self.points.len()].arrive);
                a*(1.-u).powi(3) + p*3.*(1.-u).powi(2)*u + q*3.*(1.-u)*u2 + b*u3
            }
            CurveAlgorithm::CatmullRom | CurveAlgorithm::Hermite => {
                let (m0,m1) = if self.algorithm == CurveAlgorithm::CatmullRom {
                    ((b-prev)*(1.-self.tension)*0.5, (next-a)*(1.-self.tension)*0.5)
                } else { (Vec3::from_array(self.points[i].leave), Vec3::from_array(self.points[(i+1)%self.points.len()].arrive)) };
                a*(2.*u3-3.*u2+1.) + m0*(u3-2.*u2+u) + b*(-2.*u3+3.*u2) + m1*(u3-u2)
            }
            CurveAlgorithm::BSpline => (prev*(1.-u).powi(3) + a*(3.*u3-6.*u2+4.) + b*(-3.*u3+3.*u2+3.*u+1.) + next*u3)/6.,
        };
        value.to_array()
    }
    // Open uniform B-spline with clamped end knots, degree capped at cubic.
    fn de_boor(&self, t: f32) -> Vec3 {
        let n = self.points.len(); let degree = 3.min(n-1);
        let knots: Vec<f32> = (0..n+degree+1).map(|i| {
            if i <= degree { 0. } else if i >= n { 1. } else { (i-degree) as f32/(n-degree) as f32 }
        }).collect();
        let span = (degree..n).find(|&k| t < knots[k+1]).unwrap_or(n-1);
        let mut d: Vec<Vec3> = (0..=degree).map(|j| self.position((span-degree+j) as isize)).collect();
        for r in 1..=degree { for j in (r..=degree).rev() {
            let i = span-degree+j;
            let denominator = knots[i+degree-r+1]-knots[i];
            let alpha = if denominator > 0. { (t-knots[i])/denominator } else { 0. };
            d[j] = d[j-1].lerp(d[j], alpha);
        }}
        d[degree]
    }
    pub fn samples(&self) -> Vec<[f32; 3]> {
        if self.points.len() < 2 { return self.points.iter().map(|p| p.position).collect(); }
        let count = (self.segment_count()*self.resolution.clamp(4,128) as usize).min(32768);
        (0..=count).map(|i| self.evaluate(i as f32/count as f32)).collect()
    }
    pub fn total_length_m(&self) -> f32 { self.samples().windows(2).map(|p| Vec3::from(p[0]).distance(Vec3::from(p[1]))).sum() }
    pub fn auto_tangents(&mut self) {
        let factor = if self.algorithm == CurveAlgorithm::Bezier { 1./6. } else { 0.5 };
        let tangents: Vec<_> = (0..self.points.len()).map(|i| ((self.position(i as isize+1)-self.position(i as isize-1))*factor*(1.-self.tension)).to_array()).collect();
        for (p,t) in self.points.iter_mut().zip(tangents) { p.arrive=t; p.leave=t; }
    }
    pub fn reverse(&mut self) { self.points.reverse(); for p in &mut self.points { let arrive=p.arrive; p.arrive=p.leave.map(|v|-v); p.leave=arrive.map(|v|-v); } }
    pub fn smooth(&mut self, strength: f32) {
        let positions: Vec<_> = (0..self.points.len()).map(|i| {
            let p=self.position(i as isize);
            if !self.closed && (i==0 || i+1==self.points.len()) { p.to_array() }
            else { p.lerp((self.position(i as isize-1)+self.position(i as isize+1))*0.5,strength.clamp(0.,1.)).to_array() }
        }).collect();
        for (p,v) in self.points.iter_mut().zip(positions) {p.position=v;}
        self.auto_tangents();
    }
    pub fn resample(&mut self, count: usize) {
        let samples=self.samples(); if samples.len()<2 {return;}
        let mut distance=vec![0.];
        for p in samples.windows(2) {distance.push(distance.last().unwrap()+Vec3::from(p[0]).distance(Vec3::from(p[1])));}
        let total=*distance.last().unwrap(); if total<=f32::EPSILON {return;}
        let count=count.clamp(2,512); let denominator=if self.closed {count} else {count-1};
        self.points=(0..count).map(|i| {
            let target=total*i as f32/denominator as f32;
            let j=distance.partition_point(|d| *d<target).clamp(1,samples.len()-1);
            let delta=distance[j]-distance[j-1];
            let f=if delta>0. {(target-distance[j-1])/delta} else {0.};
            SplinePoint::new(Vec3::from(samples[j-1]).lerp(Vec3::from(samples[j]),f).to_array())
        }).collect(); self.auto_tangents();
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SplineTool { #[default] Navigate, Draw, Select, Move, Insert, Delete }
impl SplineTool {
    pub const ALL: [Self;6]=[Self::Navigate,Self::Draw,Self::Select,Self::Move,Self::Insert,Self::Delete];
    pub fn key(self)->&'static str {match self {Self::Navigate=>"LevelEditor.SplinePanel.Navigate",Self::Draw=>"LevelEditor.SplinePanel.Draw",Self::Select=>"LevelEditor.SplinePanel.Select",Self::Move=>"LevelEditor.SplinePanel.Move",Self::Insert=>"LevelEditor.SplinePanel.Insert",Self::Delete=>"LevelEditor.SplinePanel.DeletePoint"}}
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DrawingPlane { #[default] XZ, XY, YZ }
impl DrawingPlane {
    pub fn normal_axis(self)->usize {match self {Self::XZ=>1,Self::XY=>2,Self::YZ=>0}}
    pub fn axes(self)->(usize,usize) {match self {Self::XZ=>(0,2),Self::XY=>(0,1),Self::YZ=>(1,2)}}
}
#[derive(Clone, Debug, PartialEq)]
pub struct SplineDomain {
    pub tool: SplineTool,
    pub selected_point: Option<usize>,
    pub selected_object: Option<String>,
    pub plane: DrawingPlane,
    pub plane_offset: f32,
    pub snap: bool,
    pub snap_step: f32,
    pub show_all: bool,
    pub show_polygon: bool,
    pub show_points: bool,
    pub show_tangents: bool,
    pub line_width: f32,
    pub preset_radius: f32,
    pub preset_count: usize,
    pub helix_height: f32,
    pub resample_count: usize,
    pub smooth_strength: f32,
}
impl Default for SplineDomain {
    fn default()->Self {Self {tool:SplineTool::Navigate,selected_point:None,selected_object:None,plane:DrawingPlane::XZ,plane_offset:0.,snap:false,snap_step:1.,show_all:true,show_polygon:true,show_points:true,show_tangents:false,line_width:2.,preset_radius:5.,preset_count:8,helix_height:10.,resample_count:16,smooth_strength:0.5}}
}

#[cfg(test)]
mod tests {
    use super::*;
    fn line()->SplineData {let mut d=SplineData::default();d.points=vec![SplinePoint::new([0.;3]),SplinePoint::new([3.,0.,4.])];d.auto_tangents();d}
    #[test] fn all_open_algorithms_preserve_endpoints() {for a in CurveAlgorithm::ALL {let mut d=line();d.algorithm=a;assert_eq!(d.evaluate(0.),[0.;3]);assert_eq!(d.evaluate(1.),[3.,0.,4.]);assert!((d.total_length_m()-5.).abs()<0.001);}}
    #[test] fn closed_curves_join_at_seam() {for a in CurveAlgorithm::ALL {let mut d=line();d.points.push(SplinePoint::new([6.,0.,0.]));d.closed=true;d.algorithm=a;assert!(Vec3::from(d.evaluate(0.)).distance(Vec3::from(d.evaluate(1.)))<0.001);}}
    #[test] fn reversing_bezier_preserves_geometry() {let mut d=line();d.algorithm=CurveAlgorithm::Bezier;d.points[0].leave=[0.,3.,0.];let original=d.clone();d.reverse();for i in 0..11 {let t=i as f32/10.;assert!(Vec3::from(d.evaluate(t)).distance(Vec3::from(original.evaluate(1.-t)))<0.001);}}
    #[test] fn resampling_is_even_and_preserves_open_endpoints() {let mut d=line();d.resample(6);assert_eq!(d.points.len(),6);for pair in d.points.windows(2) {assert!((Vec3::from(pair[0].position).distance(Vec3::from(pair[1].position))-1.).abs()<0.001);}}
    #[test] fn repeated_points_remain_finite() {let mut d=line();d.points[1]=d.points[0].clone();for a in CurveAlgorithm::ALL {d.algorithm=a;d.resample(8);assert!(d.samples().iter().flatten().all(|v|v.is_finite()));}}
}
