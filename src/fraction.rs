#[derive(Clone, Debug, Copy)]
pub struct Fraction {
    pub num: usize,
    pub den: usize,
}

fn gcd(mut a: usize, mut b: usize) -> usize {
    if a == b {
        return a;
    }
    if b > a {
        std::mem::swap(&mut a, &mut b);
    }
    while b > 0 {
        let temp = a;
        a = b;
        b = temp % b;
    }
    return a;
}

impl From<(usize, usize)> for Fraction {
    fn from(value: (usize, usize)) -> Self {
        let mut num = value.0;
        let mut den = value.1;

        let gcd = gcd(num, den);

        Self {
            num: num / gcd,
            den: den / gcd,
        }
    }
}
