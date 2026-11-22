// Records a voice message from the microphone as raw PCM through Web Audio, so it works
// without any audio codec in the webview.

export interface VoiceRecorder {
  /** Stop and return what was recorded. */
  stop: () => { samples: Float32Array; sampleRate: number };
  /** Stop and throw the recording away. */
  cancel: () => void;
  /** Seconds recorded so far. */
  elapsed: () => number;
  /** Recent input peak, 0..1. */
  level: () => number;
}

type AudioContextCtor = typeof AudioContext;

function micError(e: unknown): string {
  const name = (e as { name?: string })?.name;
  if (name === "NotAllowedError" || name === "SecurityError") return "Microphone access was denied.";
  if (name === "NotFoundError" || name === "OverconstrainedError") return "No microphone was found.";
  if (name === "NotReadableError") return "The microphone is busy or unavailable.";
  return e instanceof Error && e.message ? e.message : "Couldn't start recording.";
}

export function canRecordVoice(): boolean {
  return typeof navigator !== "undefined" && !!navigator.mediaDevices?.getUserMedia;
}

export async function startVoiceRecording(): Promise<VoiceRecorder> {
  if (!canRecordVoice()) throw new Error("Recording isn't available here. Attach an audio file instead.");
  let stream: MediaStream;
  try {
    stream = await navigator.mediaDevices.getUserMedia({
      audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true },
    });
  } catch (e) {
    throw new Error(micError(e));
  }
  const w = window as unknown as { AudioContext?: AudioContextCtor; webkitAudioContext?: AudioContextCtor };
  const Ctor = w.AudioContext ?? w.webkitAudioContext;
  if (!Ctor) {
    stream.getTracks().forEach((t) => t.stop());
    throw new Error("This webview has no Web Audio support.");
  }
  const ctx = new Ctor();
  await ctx.resume().catch(() => {});
  const source = ctx.createMediaStreamSource(stream);
  // ScriptProcessorNode is deprecated but needs no worklet module (blocked by the CSP).
  const proc = ctx.createScriptProcessor(4096, 1, 1);
  const chunks: Float32Array[] = [];
  let length = 0;
  let peak = 0;
  proc.onaudioprocess = (e) => {
    const data = e.inputBuffer.getChannelData(0);
    chunks.push(new Float32Array(data));
    length += data.length;
    let p = 0;
    for (let i = 0; i < data.length; i++) p = Math.max(p, Math.abs(data[i]));
    peak = p;
    e.outputBuffer.getChannelData(0).fill(0);
  };
  source.connect(proc);
  proc.connect(ctx.destination);
  let closed = false;
  const close = () => {
    if (closed) return;
    closed = true;
    proc.onaudioprocess = null;
    proc.disconnect();
    source.disconnect();
    stream.getTracks().forEach((t) => t.stop());
    void ctx.close().catch(() => {});
  };
  return {
    elapsed: () => length / ctx.sampleRate,
    level: () => peak,
    cancel: close,
    stop: () => {
      close();
      const samples = new Float32Array(length);
      let o = 0;
      for (const c of chunks) {
        samples.set(c, o);
        o += c.length;
      }
      return { samples, sampleRate: ctx.sampleRate };
    },
  };
}
