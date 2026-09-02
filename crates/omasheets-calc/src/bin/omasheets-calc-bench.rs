use omasheets_calc::{CellId, Value, Workbook};
use std::env;
use std::hint::black_box;
use std::process::ExitCode;
use std::time::Instant;

const DEFAULT_FORMULAS: usize = 100_000;
const DEFAULT_ITERATIONS: usize = 20;
const MAX_FORMULAS: usize = 1_000_000;
const MAX_ITERATIONS: usize = 10_000;

fn usage() -> &'static str {
    "usage: omasheets-calc-bench [--formulas N] [--iterations N]"
}

fn bounded_number(value: Option<String>, name: &str, maximum: usize) -> Result<usize, String> {
    let value = value.ok_or_else(|| format!("missing value for {name}"))?;
    value
        .parse::<usize>()
        .ok()
        .filter(|number| *number > 0 && *number <= maximum)
        .ok_or_else(|| format!("{name} must be between 1 and {maximum}"))
}

fn parse_arguments() -> Result<(usize, usize), String> {
    let mut formulas = DEFAULT_FORMULAS;
    let mut iterations = DEFAULT_ITERATIONS;
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--formulas" => {
                formulas = bounded_number(arguments.next(), "--formulas", MAX_FORMULAS)?;
            }
            "--iterations" => {
                iterations = bounded_number(arguments.next(), "--iterations", MAX_ITERATIONS)?;
            }
            "--help" | "-h" => return Err(usage().into()),
            _ => return Err(format!("unknown argument {argument}")),
        }
    }
    Ok((formulas, iterations))
}

fn percentile(sorted: &[u128], percentile: usize) -> u128 {
    let index = (sorted.len() * percentile).div_ceil(100).saturating_sub(1);
    sorted[index]
}

fn run(formulas: usize, iterations: usize) -> Result<(), String> {
    let root = CellId::new(0, 0, 0);
    let mut workbook = Workbook::default();
    let build_started = Instant::now();
    workbook.set_number(root, 1.0);
    for row in 1..=formulas {
        workbook
            .set_formula(
                CellId::new(0, u32::try_from(row).map_err(|_| "row overflow")?, 0),
                &format!("=A{row} + 1"),
            )
            .map_err(|error| error.to_string())?;
    }
    let build_ns = build_started.elapsed().as_nanos();

    let mut samples_ns = Vec::with_capacity(iterations);
    let mut evaluated = 0;
    for iteration in 0..iterations {
        let started = Instant::now();
        let report = workbook.set_number(root, black_box(iteration as f64 + 2.0));
        samples_ns.push(started.elapsed().as_nanos());
        evaluated = report.evaluated.len();
    }
    samples_ns.sort_unstable();
    let output = workbook.value(CellId::new(0, formulas as u32, 0));
    let output = match output {
        Value::Number(number) => number,
        other => return Err(format!("benchmark produced {other:?}")),
    };

    println!(
        concat!(
            "{{\n",
            "  \"schema\": 1,\n",
            "  \"fixture\": \"linear_dirty_closure\",\n",
            "  \"target_os\": \"{target_os}\",\n",
            "  \"target_arch\": \"{target_arch}\",\n",
            "  \"formulas\": {formulas},\n",
            "  \"iterations\": {iterations},\n",
            "  \"cells_evaluated_per_edit\": {evaluated},\n",
            "  \"build_ns\": {build_ns},\n",
            "  \"edit_recalc_p50_ns\": {p50},\n",
            "  \"edit_recalc_p95_ns\": {p95},\n",
            "  \"edit_recalc_max_ns\": {maximum},\n",
            "  \"last_value\": {output}\n",
            "}}"
        ),
        target_os = env::consts::OS,
        target_arch = env::consts::ARCH,
        formulas = formulas,
        iterations = iterations,
        evaluated = evaluated,
        build_ns = build_ns,
        p50 = percentile(&samples_ns, 50),
        p95 = percentile(&samples_ns, 95),
        maximum = samples_ns[samples_ns.len() - 1],
        output = output,
    );
    Ok(())
}

fn main() -> ExitCode {
    match parse_arguments().and_then(|(formulas, iterations)| run(formulas, iterations)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("omasheets-calc-bench: {error}\n{}", usage());
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_percentiles_are_deterministic() {
        let samples = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        assert_eq!(percentile(&samples, 50), 5);
        assert_eq!(percentile(&samples, 95), 10);
    }
}
