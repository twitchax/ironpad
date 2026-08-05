//! OTLP export wiring: tracer/logger providers, the log-export filter, and
//! the endpoint-driven enablement switch.
//!
//! Split out of `main.rs` (PRD-0055 T-003); behavior unchanged.

/// Tracer + logger providers for OTLP export, held for the process lifetime so
/// their batch exporters keep flushing.
pub(crate) struct OtelProviders {
    pub(crate) tracer: opentelemetry_sdk::trace::SdkTracerProvider,
    pub(crate) logger: opentelemetry_sdk::logs::SdkLoggerProvider,
}

/// Build the OTLP tracer + logger providers when OpenTelemetry export is
/// configured.
///
/// Opt-in: returns `None` unless `OTEL_EXPORTER_OTLP_ENDPOINT` is set, so the
/// server keeps its plain stdout logging until an endpoint is provided. Both
/// exporters read the endpoint and auth headers from the standard
/// `OTEL_EXPORTER_OTLP_*` env vars, so no credentials live in code or config.
/// The shared `service.name` resource is what lets Grafana correlate the two
/// signals.
pub(crate) fn init_otel() -> Option<OtelProviders> {
    if !otel_export_enabled(std::env::var_os("OTEL_EXPORTER_OTLP_ENDPOINT").as_deref()) {
        return None;
    }

    // The subscriber isn't initialized yet, so stderr is the only channel for
    // exporter-build failures.
    let span_exporter = match opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .build()
    {
        Ok(exporter) => exporter,
        Err(err) => {
            eprintln!("OTLP span exporter init failed; export disabled: {err}");
            return None;
        }
    };
    let log_exporter = match opentelemetry_otlp::LogExporter::builder()
        .with_http()
        .build()
    {
        Ok(exporter) => exporter,
        Err(err) => {
            eprintln!("OTLP log exporter init failed; export disabled: {err}");
            return None;
        }
    };

    let resource = opentelemetry_sdk::Resource::builder()
        .with_service_name("ironpad-server")
        .build();

    let tracer = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_batch_exporter(span_exporter)
        .with_resource(resource.clone())
        .build();
    let logger = opentelemetry_sdk::logs::SdkLoggerProvider::builder()
        .with_batch_exporter(log_exporter)
        .with_resource(resource)
        .build();

    Some(OtelProviders { tracer, logger })
}

/// Per-layer filter for the OTLP log bridge: forward application logs but drop
/// the telemetry stack's own events, so exporting a log can't emit a log that
/// gets exported again (a feedback loop).
pub(crate) fn otel_log_filter() -> tracing_subscriber::filter::Targets {
    use tracing_subscriber::filter::LevelFilter;
    tracing_subscriber::filter::Targets::new()
        .with_default(LevelFilter::TRACE)
        .with_target("opentelemetry", LevelFilter::OFF)
        .with_target("hyper", LevelFilter::OFF)
        .with_target("hyper_util", LevelFilter::OFF)
        .with_target("reqwest", LevelFilter::OFF)
        .with_target("h2", LevelFilter::OFF)
        .with_target("rustls", LevelFilter::OFF)
}

/// OTLP export is opt-in on a configured endpoint. Pulled out as a pure helper
/// so the gating contract is unit-testable without touching process env.
fn otel_export_enabled(endpoint: Option<&std::ffi::OsStr>) -> bool {
    endpoint.is_some()
}

#[cfg(test)]
mod tests {
    use super::otel_export_enabled;

    #[test]
    fn otel_export_is_off_without_an_endpoint() {
        assert!(!otel_export_enabled(None));
    }

    #[test]
    fn otel_export_is_on_with_an_endpoint() {
        assert!(otel_export_enabled(Some(std::ffi::OsStr::new(
            "https://otlp-gateway.grafana.net/otlp"
        ))));
    }
}
