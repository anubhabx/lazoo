// Runs on the audio thread. Collects the first input channel into blocks and
// posts them to the page. Served as a real file from the app origin, because the
// app's CSP (`script-src 'self'`) refuses a worklet built from a Blob or data URL.

const BLOCK = 1024;

class LazooCapture extends AudioWorkletProcessor {
  constructor() {
    super();
    this.block = new Float32Array(BLOCK);
    this.filled = 0;
    this.port.onmessage = (event) => {
      if (event.data !== "flush") return;
      if (this.filled > 0)
        this.port.postMessage(this.block.slice(0, this.filled));
      this.filled = 0;
      this.port.postMessage("flushed");
    };
  }

  process(inputs) {
    const channel = inputs[0] && inputs[0][0];
    if (channel) {
      for (let index = 0; index < channel.length; index += 1) {
        this.block[this.filled] = channel[index];
        this.filled += 1;
        if (this.filled === BLOCK) {
          this.port.postMessage(this.block.slice(0));
          this.filled = 0;
        }
      }
    }
    return true;
  }
}

registerProcessor("lazoo-capture", LazooCapture);
