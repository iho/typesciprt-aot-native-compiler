// Regression test: Set.add(value) with exactly 1 arg must still route through
// ts_container_add (not fall through to dynamic method dispatch).
const s = new Set<number>();
s.add(10);
s.add(20);
s.add(30);
s.add(20); // duplicate — set stays at 3
s.size     // 3
