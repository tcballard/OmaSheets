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
    "usage: omasheets-calc-bench [--formulas N] [--iterations N] [--fixture linear|fan_out|sparse]"
}

/// The dependency shape the synthetic document is built with.
///
/// `linear` chains every formula on the previous one, so one root edit
/// re-evaluates the whole document: the worst case for closure size and the
/// measurement the M0 recalculation target is judged on. `fan_out` points
/// every formula at the root, so one edit re-evaluates everything with no
/// chain depth. `sparse` builds `formulas / 1000` independent chains of 1000
/// and edits one chain's root, so one edit re-evaluates about 1000 cells,
/// which is closer to an ordinary keystroke in a real workbook.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fixture {
    Linear,
    FanOut,
    Sparse,
}

impl Fixture {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "linear" => Ok(Self::Linear),
            "fan_out" => Ok(Self::FanOut),
            "sparse" => Ok(Self::Sparse),
            other => Err(format!("unknown fixture {other}")),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Linear => "linear_dirty_closure",
            Self::FanOut => "fan_out_dirty_closure",
            Self::Sparse => "sparse_chain_edit",
        }
    }
}

const SPARSE_CHAIN_LENGTH: usize = 1000;

fn bounded_number(value: Option<String>, name: &str, maximum: usize) -> Result<usize, String> {
    let value = value.ok_or_else(|| format!("missing value for {name}"))?;
    value
        .parse::<usize>()
        .ok()
        .filter(|number| *number > 0 && *number <= maximum)
        .ok_or_else(|| format!("{name} must be between 1 and {maximum}"))
}

fn parse_arguments() -> Result<(usize, usize, Fixture), String> {
    let mut formulas = DEFAULT_FORMULAS;
    let mut iterations = DEFAULT_ITERATIONS;
    let mut fixture = Fixture::Linear;
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--formulas" => {
                formulas = bounded_number(arguments.next(), "--formulas", MAX_FORMULAS)?;
            }
            "--iterations" => {
                iterations = bounded_number(arguments.next(), "--iterations", MAX_ITERATIONS)?;
            }
            "--fixture" => {
                let value = arguments.next().ok_or("missing value for --fixture")?;
                fixture = Fixture::parse(&value)?;
            }
            "--help" | "-h" => return Err(usage().into()),
            _ => return Err(format!("unknown argument {argument}")),
        }
    }
    if fixture == Fixture::Sparse && formulas < SPARSE_CHAIN_LENGTH {
        return Err(format!(
            "--fixture sparse needs at least {SPARSE_CHAIN_LENGTH} formulas"
        ));
    }
    Ok((formulas, iterations, fixture))
}

fn percentile(sorted: &[u128], percentile: usize) -> u128 {
    let index = (sorted.len() * percentile).div_ceil(100).saturating_sub(1);
    sorted[index]
}

fn cell(row: usize) -> Result<CellId, String> {
    Ok(CellId::new(
        0,
        u32::try_from(row).map_err(|_| "row overflow")?,
        0,
    ))
}

/// Builds the fixture and returns the cell one edit is applied to and the
/// cell whose final value proves the whole closure was evaluated.
fn build(
    workbook: &mut Workbook,
    formulas: usize,
    fixture: Fixture,
) -> Result<(CellId, CellId), String> {
    let root = CellId::new(0, 0, 0);
    match fixture {
        Fixture::Linear => {
            workbook.set_number(root, 1.0);
            for row in 1..=formulas {
                workbook
                    .set_formula(cell(row)?, &format!("=A{row} + 1"))
                    .map_err(|error| error.to_string())?;
            }
            Ok((root, cell(formulas)?))
        }
        Fixture::FanOut => {
            workbook.set_number(root, 1.0);
            for row in 1..=formulas {
                workbook
                    .set_formula(cell(row)?, "=A1 + 1")
                    .map_err(|error| error.to_string())?;
            }
            Ok((root, cell(formulas)?))
        }
        Fixture::Sparse => {
            // Chain k occupies rows k*1000 .. k*1000+999; row k*1000 is its
            // literal root. Every chain is independent of every other.
            let chains = formulas / SPARSE_CHAIN_LENGTH;
            for chain in 0..chains {
                let base = chain * SPARSE_CHAIN_LENGTH;
                workbook.set_number(cell(base)?, 1.0);
                for offset in 1..SPARSE_CHAIN_LENGTH {
                    let row = base + offset;
                    workbook
                        .set_formula(cell(row)?, &format!("=A{row} + 1"))
                        .map_err(|error| error.to_string())?;
                }
            }
            let edited = chains / 2 * SPARSE_CHAIN_LENGTH;
            Ok((cell(edited)?, cell(edited + SPARSE_CHAIN_LENGTH - 1)?))
        }
    }
}

fn run(formulas: usize, iterations: usize, fixture: Fixture) -> Result<(), String> {
    let mut workbook = Workbook::default();
    let build_started = Instant::now();
    let (edited, probe) = build(&mut workbook, formulas, fixture)?;
    let build_ns = build_started.elapsed().as_nanos();

    let mut samples_ns = Vec::with_capacity(iterations);
    let mut evaluated = 0;
    for iteration in 0..iterations {
        let started = Instant::now();
        let report = workbook.set_number(edited, black_box(iteration as f64 + 2.0));
        samples_ns.push(started.elapsed().as_nanos());
        evaluated = report.evaluated.len();
    }
    samples_ns.sort_unstable();
    let output = match workbook.value(probe) {
        Value::Number(number) => number,
        other => return Err(format!("benchmark produced {other:?}")),
    };

    println!(
        concat!(
            "{{\n",
            "  \"schema\": 1,\n",
            "  \"fixture\": \"{fixture}\",\n",
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
        fixture = fixture.name(),
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
    match parse_arguments()
        .and_then(|(formulas, iterations, fixture)| run(formulas, iterations, fixture))
    {
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

    #[test]
    fn fixtures_evaluate_the_closure_they_claim() {
        for (fixture, formulas, expected_closure, expected_probe) in [
            // Root 1.0 becomes 7.0; the chain end is root + formulas.
            (Fixture::Linear, 5_000, 5_001, 7.0 + 5_000.0),
            // Every formula is root + 1.
            (Fixture::FanOut, 5_000, 5_001, 8.0),
            // Only the edited chain of 1000 re-evaluates; its end is root + 999.
            (Fixture::Sparse, 5_000, 1_000, 7.0 + 999.0),
        ] {
            let mut workbook = Workbook::default();
            let (edited, probe) = build(&mut workbook, formulas, fixture).unwrap();
            let report = workbook.set_number(edited, 7.0);
            assert_eq!(report.evaluated.len(), expected_closure, "{fixture:?}");
            assert_eq!(
                workbook.value(probe),
                Value::Number(expected_probe),
                "{fixture:?}"
            );
        }
    }

    #[test]
    fn fixture_names_parse_and_sparse_needs_a_full_chain() {
        assert_eq!(Fixture::parse("linear").unwrap(), Fixture::Linear);
        assert_eq!(Fixture::parse("fan_out").unwrap(), Fixture::FanOut);
        assert_eq!(Fixture::parse("sparse").unwrap(), Fixture::Sparse);
        assert!(Fixture::parse("tree").is_err());
        let mut workbook = Workbook::default();
        assert!(build(&mut workbook, SPARSE_CHAIN_LENGTH, Fixture::Sparse).is_ok());
    }
}
