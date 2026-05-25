//コマンドライン引数の処理を行うファイル
use anyhow::{Result, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
//Eq:
// PartialEq:
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
//<I, S>はジェネリティクス: 後で型を決める
pub fn parse_args<I, S>(args: I) -> Result<RuntimeOptions>
where//ジェネリティクスの型に制約をつける
    I: IntoIterator<Item = S>,//iteratorに変換できる型,Itemはイテレータを一回回したら出てくる要素の型
    S: AsRef<str>,//&strとして参照できる型
{
    let mut options = RuntimeOptions::default();

    for arg in args {
        match arg.as_ref() {//as_ref()はAsRef<T>で定義したTに変換して返す
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
