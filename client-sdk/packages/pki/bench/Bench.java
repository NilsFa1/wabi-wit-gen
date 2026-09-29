// Shared measurement helpers for the gematik:pki benchmarks (Java variants).
import java.util.Arrays;
import java.util.concurrent.Callable;
import java.util.concurrent.Executors;

final class Bench {
    private Bench() {}

    interface Op { void run() throws Exception; }

    static final int WARMUP = Integer.getInteger("bench.warmup", 3000);
    static final int N = Integer.getInteger("bench.n", 5000);
    static final int THREADS = 8;

    /** Sequential latency after warm-up: mean / p50 / p99 in microseconds. */
    static void latency(String name, Op op) throws Exception {
        for (int i = 0; i < WARMUP; i++) op.run();
        long[] t = new long[N];
        for (int i = 0; i < N; i++) {
            long s = System.nanoTime();
            op.run();
            t[i] = System.nanoTime() - s;
        }
        Arrays.sort(t);
        double mean = Arrays.stream(t).average().orElse(0) / 1000.0;
        System.out.printf("%-34s mean %8.1f µs   p50 %8.1f µs   p99 %8.1f µs%n", name, mean, t[N / 2] / 1000.0, t[N * 99 / 100] / 1000.0);
    }

    /** Throughput with THREADS callers, each doing N/THREADS*2 operations. */
    static void throughput(String name, Op op) throws Exception {
        int per = N * 2 / THREADS;
        try (var pool = Executors.newFixedThreadPool(THREADS)) {
            Callable<Void> task = () -> { for (int i = 0; i < per; i++) op.run(); return null; };
            var warm = new java.util.ArrayList<java.util.concurrent.Future<Void>>();
            for (int i = 0; i < THREADS; i++) warm.add(pool.submit(task));
            for (var f : warm) f.get();
            long s = System.nanoTime();
            var all = new java.util.ArrayList<java.util.concurrent.Future<Void>>();
            for (int i = 0; i < THREADS; i++) all.add(pool.submit(task));
            for (var f : all) f.get();
            double secs = (System.nanoTime() - s) / 1e9;
            System.out.printf("%-34s %8.0f ops/s (%d threads)%n", name, per * THREADS / secs, THREADS);
        }
    }

    static void once(String name, Op op) throws Exception {
        long s = System.nanoTime();
        op.run();
        System.out.printf("%-34s %8.1f ms%n", name, (System.nanoTime() - s) / 1e6);
    }
}
