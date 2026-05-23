#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chunk {
    pub start: u64,
    pub end: u64,
}

impl Chunk {
    pub fn len(self) -> u64 {
        self.end - self.start + 1
    }
}

pub fn split_in_half(total_size: u64) -> [Chunk; 2] {
    assert!(total_size >= 2);

    let mid = total_size / 2;
    [
        Chunk {
            start: 0,
            end: mid - 1,
        },
        Chunk {
            start: mid,
            end: total_size - 1,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_even_size() {
        assert_eq!(
            split_in_half(10),
            [Chunk { start: 0, end: 4 }, Chunk { start: 5, end: 9 },]
        );
    }

    #[test]
    fn puts_extra_byte_in_second_chunk_for_odd_size() {
        assert_eq!(
            split_in_half(11),
            [Chunk { start: 0, end: 4 }, Chunk { start: 5, end: 10 },]
        );
    }
}
