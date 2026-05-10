use anyhow::{Result, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeOptions {
    pub range_split_enabled: bool,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            range_split_enabled: false,
        }
    }
}

pub fn parse_args<I, S>(args: I) -> Result<RuntimeOptions>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut options = RuntimeOptions::default();

    for arg in args {
        match arg.as_ref() {
            "--range-split" => options.range_split_enabled = true,
            "--no-range-split" => options.range_split_enabled = false,
            "--help" | "-h" => bail!(
                "usage: mnic-cli [--range-split|--no-range-split]\n\
                 default: range split is OFF"
            ),
            unknown => bail!("unknown argument: {unknown}"),
        }
    }

    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_range_split_to_off() {
        let options = parse_args(std::iter::empty::<&str>()).unwrap();
        assert!(!options.range_split_enabled);
    }

    #[test]
    fn enables_range_split_when_requested() {
        let options = parse_args(["--range-split"]).unwrap();
        assert!(options.range_split_enabled);
    }

    #[test]
    fn disables_range_split_when_requested() {
        let options = parse_args(["--range-split", "--no-range-split"]).unwrap();
        assert!(!options.range_split_enabled);
    }

    #[test]
    fn rejects_unknown_arguments() {
        let error = parse_args(["--csv"]).unwrap_err();
        assert!(error.to_string().contains("unknown argument"));
    }
}
