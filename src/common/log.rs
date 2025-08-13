use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, Registry};
use tracing_timing::{Histogram, group};
use tracing_tree::time::UtcDateTime;

pub fn init_logging() {
    tracing_subscriber::registry()
        .with(tree_layer())
        .with(timing_layer())
        .with(EnvFilter::from_default_env())
        .init();
}

pub fn tree_layer() -> impl Layer<Registry> {
    tracing_tree::HierarchicalLayer::default()
        .with_indent_amount(2)
        .with_indent_lines(true)
        .with_deferred_spans(true)
        .with_span_retrace(true)
        .with_targets(true)
        .with_timer(UtcDateTime::default())
}

type TimingLayer = tracing_timing::TimingLayer<group::ByName, group::ByMessage>;

fn timing_layer() -> TimingLayer {
    tracing_timing::Builder::default()
        //.events(group::ByName)
        .layer(|| Histogram::new_with_max(100_000_000, 2).unwrap())
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum MetricsCommand {
    ShowTiming,
}

pub fn handle_command(command: MetricsCommand) {
    match command {
        MetricsCommand::ShowTiming => show_timing(),
    }
}

pub fn show_timing() {
    tracing::dispatcher::get_default(|d| {
        let timing_layer = d.downcast_ref::<TimingLayer>().unwrap();
        print_histograms(timing_layer);
    })
}

fn print_histograms(timing_layer: &TimingLayer) {
    timing_layer.force_synchronize();
    timing_layer.with_histograms(|hs| {
        println!("\nHistograms:\n");
        for (span, hs) in hs {
            for (event, h) in hs {
                let ns = |nanos| Duration::from_nanos(nanos);
                println!("{span} -> {event} ({} events)", h.len());
                println!("    mean: {:?}", ns(h.mean() as u64));
                println!("    min: {:?}", ns(h.min()));
                println!("    p50: {:?}", ns(h.value_at_quantile(0.50)));
                println!("    p90: {:?}", ns(h.value_at_quantile(0.90)));
                println!("    p99: {:?}", ns(h.value_at_quantile(0.99)));
                println!("    max: {:?}", ns(h.max()));
            }
        }
        println!();
    });
}

pub fn trace_misc<T>(desc: &str, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let out = f();
    let end = Instant::now();
    tracing::trace!(time = ?(end - start), "{desc}");
    out
}
