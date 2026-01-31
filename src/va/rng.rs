pub struct Rng {
    s0: usize,
    s1: usize,
}

impl Rng {
    pub const fn new(seed: usize) -> Self {
        let mut rng = Self { s0: 0, s1: 0 };
        rng.s0 = seed;
        rng.s1 = (seed << 32) ^ seed;
        rng
    }

    #[inline(always)]
    pub fn next_usize(&mut self) -> usize {
        let mut x = self.s0;
        let y = self.s1;

        x ^= x << 23;
        x ^= x >> 17;
        x ^= y ^ (y >> 26);

        self.s0 = y;
        self.s1 = x;

        let z = x.wrapping_add(y);
        self.s0 ^= z.rotate_left(17);

        let mut out = z;
        out ^= out >> 30;
        out = out.wrapping_mul(0xbf58476d1ce4e5b9);
        out ^= out >> 27;
        out = out.wrapping_mul(0x94d049bb133111eb);
        out ^= out >> 31;

        out
    }
}
