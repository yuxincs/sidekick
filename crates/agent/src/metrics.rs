use host_agent_core::{config::Exposure, protocol::Metrics, service::MetricsProvider};
use sysinfo::System;

pub struct HostMetrics {
    system: System,
    cpu_primed: bool,
}

impl HostMetrics {
    pub fn new() -> Self {
        Self {
            system: System::new(),
            cpu_primed: false,
        }
    }
}

impl MetricsProvider for HostMetrics {
    fn sample(&mut self, exposure: Exposure) -> Metrics {
        let mut metrics = Metrics::default();
        if exposure.cpu {
            self.system.refresh_cpu_usage();
            if self.cpu_primed {
                metrics.cpu_percent = Some(self.system.global_cpu_usage());
            }
            self.cpu_primed = true;
        } else {
            self.cpu_primed = false;
        }
        if exposure.memory {
            self.system.refresh_memory();
            let total = self.system.total_memory();
            if total > 0 {
                metrics.memory_percent =
                    Some(100.0 * self.system.used_memory() as f32 / total as f32);
            }
        }
        metrics
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disabled_collectors_return_no_data() {
        let mut provider = HostMetrics::new();
        let data = provider.sample(Exposure::default());
        assert!(data.cpu_percent.is_none());
        assert!(data.memory_percent.is_none());
        assert!(provider.system.cpus().is_empty());
    }
    #[test]
    fn cpu_is_warmed_up_and_ram_is_a_percentage() {
        let mut provider = HostMetrics::new();
        let enabled = Exposure {
            cpu: true,
            memory: true,
            shutdown: false,
            ..Exposure::default()
        };
        let data = provider.sample(enabled);
        assert!(data.cpu_percent.is_none());
        assert!((0.0..=100.0).contains(&data.memory_percent.unwrap()));
        std::thread::sleep(
            sysinfo::MINIMUM_CPU_UPDATE_INTERVAL + std::time::Duration::from_millis(10),
        );
        assert!((0.0..=100.0).contains(&provider.sample(enabled).cpu_percent.unwrap()));
        provider.sample(Exposure::default());
        assert!(provider.sample(enabled).cpu_percent.is_none());
    }
}
