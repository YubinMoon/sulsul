use std::time::Duration;

/// 모노 f32 PCM 오디오 (-1.0 ~ 1.0).
#[derive(Debug, Clone, PartialEq)]
pub struct AudioClip {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

impl AudioClip {
    pub fn new(samples: Vec<f32>, sample_rate: u32) -> Self {
        Self {
            samples,
            sample_rate,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn duration(&self) -> Duration {
        if self.sample_rate == 0 {
            return Duration::ZERO;
        }
        Duration::from_secs_f64(self.samples.len() as f64 / f64::from(self.sample_rate))
    }

    /// 절댓값 최대치 (0.0 ~ 1.0)
    pub fn peak(&self) -> f32 {
        self.samples.iter().fold(0.0_f32, |acc, s| acc.max(s.abs()))
    }

    /// 제곱평균제곱근 음량 (0.0 ~ 1.0)
    pub fn rms(&self) -> f32 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let sum: f64 = self.samples.iter().map(|s| f64::from(*s).powi(2)).sum();
        (sum / self.samples.len() as f64).sqrt() as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_and_levels() {
        let clip = AudioClip::new(vec![0.0, 0.5, -1.0, 0.5], 4);
        assert_eq!(clip.duration(), Duration::from_secs(1));
        assert_eq!(clip.peak(), 1.0);
        assert!((clip.rms() - 0.612_372_4).abs() < 1e-6);
    }

    #[test]
    fn empty_clip() {
        let clip = AudioClip::new(Vec::new(), 16_000);
        assert!(clip.is_empty());
        assert_eq!(clip.duration(), Duration::ZERO);
        assert_eq!(clip.rms(), 0.0);
    }
}
