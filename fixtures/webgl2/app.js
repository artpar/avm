(() => {
  const surface = document.querySelector('#surface');
  const status = document.querySelector('#status');
  const gl = surface.getContext('webgl2', {
    alpha: false,
    antialias: false,
    depth: false,
    preserveDrawingBuffer: true,
    stencil: false,
  });

  if (!gl) {
    document.title = 'AVM_WEBGL2_FAIL';
    status.textContent = 'WEBGL2_UNAVAILABLE';
    console.error('avm.webgl2.unavailable');
    return;
  }

  const renderer = gl.getParameter(gl.RENDERER);
  const version = gl.getParameter(gl.VERSION);
  let updated = false;

  function resizeAndClear() {
    const width = Math.max(1, Math.floor(innerWidth * devicePixelRatio));
    const height = Math.max(1, Math.floor(innerHeight * devicePixelRatio));
    if (surface.width !== width || surface.height !== height) {
      surface.width = width;
      surface.height = height;
    }
    gl.viewport(0, 0, surface.width, surface.height);
    if (updated) {
      gl.clearColor(230 / 255, 51 / 255, 204 / 255, 1);
      document.title = 'AVM_WEBGL2_UPDATED';
    } else {
      gl.clearColor(32 / 255, 191 / 255, 64 / 255, 1);
      document.title = 'AVM_WEBGL2_OK';
    }
    gl.clear(gl.COLOR_BUFFER_BIT);
    status.textContent = `${document.title} renderer=${renderer} version=${version}`;
  }

  surface.addEventListener('pointerdown', () => {
    updated = true;
    resizeAndClear();
    console.info('avm.webgl2.updated', { renderer, version });
  });
  addEventListener('resize', resizeAndClear);
  resizeAndClear();
  console.info('avm.webgl2.ready', { renderer, version });
})();
