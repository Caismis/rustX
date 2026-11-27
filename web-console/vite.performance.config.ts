import config from './vite.config';

export default {
  ...config,
  test: { ...config.test, include: ['test/incremental-performance.measurement.tsx'], maxWorkers: 1, fileParallelism: false },
};
