use serde_json::{Value, json};

#[derive(Default)]
pub struct Quality {
    pub images: u64,
    pub pixels: u64,
    pub channel_max: [u8; 4],
    pub channel_sum: [u64; 4],
    pub channel_histograms: Vec<[u64; 256]>,
    pub over_ten: u64,
    pub angle_max: f64,
    pub angle_sum: f64,
    pub angles: Vec<u64>,
}
impl Quality {
    pub fn add(&mut self, reference: &[u8], decoded: &[u8], normal: bool) {
        assert_eq!(reference.len(), decoded.len());
        self.images += 1;
        if self.channel_histograms.is_empty() {
            self.channel_histograms.resize(4, [0; 256]);
        }
        if normal && self.angles.is_empty() {
            self.angles.resize(180_001, 0);
        }
        for (a, b) in reference
            .as_chunks::<4>()
            .0
            .iter()
            .zip(decoded.as_chunks::<4>().0)
        {
            self.pixels += 1;
            for c in 0..4 {
                let d = a[c].abs_diff(b[c]);
                self.channel_max[c] = self.channel_max[c].max(d);
                self.channel_sum[c] += u64::from(d);
                self.channel_histograms[c][d as usize] += 1;
            }
            if normal {
                let vector = |p: &[u8; 4]| {
                    let v = [
                        p[0] as f64 / 127.5 - 1.,
                        p[1] as f64 / 127.5 - 1.,
                        p[2] as f64 / 127.5 - 1.,
                    ];
                    let length = v.iter().map(|v| v * v).sum::<f64>().sqrt();
                    v.map(|v| v / length)
                };
                let (a, b) = (vector(a), vector(b));
                let angle = a
                    .iter()
                    .zip(b)
                    .map(|(a, b)| a * b)
                    .sum::<f64>()
                    .clamp(-1., 1.)
                    .acos()
                    .to_degrees();
                self.angle_max = self.angle_max.max(angle);
                self.angle_sum += angle;
                self.over_ten += u64::from(angle > 10.);
                self.angles[(angle * 1000.).ceil() as usize] += 1;
            }
        }
    }
    pub fn report(&self) -> Value {
        let threshold = (self.pixels * 99).div_ceil(100);
        let mut count = 0;
        let p99 = self
            .angles
            .iter()
            .position(|n| {
                count += n;
                count >= threshold
            })
            .map(|i| i as f64 / 1000.);
        let channel_p99: Vec<_> = self
            .channel_histograms
            .iter()
            .map(|hist| {
                let mut count = 0;
                hist.iter().position(|n| {
                    count += n;
                    count >= threshold
                })
            })
            .collect();
        json!({"distinct_images":self.images,"pixels":self.pixels,"channel_max_rgba":self.channel_max,"channel_mean_rgba":self.channel_sum.map(|v|v as f64/self.pixels as f64),"channel_p99_rgba":channel_p99,"normal_angle_max_deg":self.angle_max,"normal_angle_mean_deg":self.angle_sum/self.pixels as f64,"normal_angle_p99_upper_deg":p99,"normal_over_10_deg":self.over_ten,"normal_over_10_share":self.over_ten as f64/self.pixels as f64,"p99_bin_deg":0.001})
    }
}
