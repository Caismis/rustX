import config from './vite.config.ts';

export default {
  ...config,
  test: { ...config.test, include: ['test/incremental-performance.measurement.tsx'], maxWorkers: 1, fileParallelism: false, testTimeout: 30_000 },
};
