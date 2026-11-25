/** One directory query on the private, launcher-authenticated Product Host lane. */
export function browseNativeWorkspace(endpoint: string, credential: string, root: string, directory: string,
  expected: string | undefined, signal: AbortSignal): Promise<{ folders: string[]; identity: string; revision: string }> {
  signal.throwIfAborted();
  const url = new URL(endpoint);
  if (url.pathname !== '/' || url.search || url.hash || url.username || url.password) throw new Error('Invalid Product Host native endpoint');
  url.pathname = '/product-host/workspace-picker';
  const socket = new WebSocket(url, ['rustx.product-host.workspace-picker.v1', `rustx-product-host.${credential}`]);
  return new Promise((resolve, reject) => {
    const finish = () => { signal.removeEventListener('abort', abort); socket.close(); };
    const abort = () => { finish(); reject(signal.reason); };
    signal.addEventListener('abort', abort, { once: true });
    socket.addEventListener('open', () => {
      if (signal.aborted) return abort();
      try { socket.send(JSON.stringify({ root, directory, expected })); }
      catch (error) { finish(); reject(error); }
    }, { once: true });
    socket.addEventListener('message', event => {
      try {
        if (signal.aborted) return abort();
        if (typeof event.data !== 'string' || Buffer.byteLength(event.data) > 1_048_576) throw new Error('Invalid directory response');
        const response = JSON.parse(event.data);
        if (typeof response?.Err === 'string') throw new Error(response.Err);
        const listing = response?.Ok;
        if (!Array.isArray(listing?.folders) || listing.folders.length > 4096
          || listing.folders.some((name: unknown) => typeof name !== 'string' || !name || name === '.' || name === '..' || /[\/\0]/.test(name))
          || typeof listing.identity !== 'string' || !/^\d+:\d+$/.test(listing.identity)
          || typeof listing.revision !== 'string' || !/^[a-f0-9]{64}$/.test(listing.revision)) throw new Error('Invalid directory response');
        resolve(listing);
      } catch (error) { reject(error); }
      finally { finish(); }
    }, { once: true });
    const unavailable = () => {
      finish();
      reject(new Error('The local Execution Client could not browse this location.'));
    };
    socket.addEventListener('error', unavailable, { once: true });
    socket.addEventListener('close', unavailable, { once: true });
  });
}
