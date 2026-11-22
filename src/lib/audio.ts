// Browser-side audio decoding for files the backend cannot decode ("needsDecode").
// The webview decodes with Web Audio, we downmix to mono and resample to 16 kHz,
// then hand raw float32 PCM to the backend.

export const TRANSCRIPTION_SAMPLE_RATE = 16000;

/** Average all channels into one. Channels may differ in length (shorter ones pad with 0). */
export function downmixToMono(channels: readonly Float32Array[]): Float32Array {
  if (channels.length === 0) return new Float32Array(0);
  if (channels.length === 1) return new Float32Array(channels[0]);
  const length = Math.max(...channels.map((c) => c.length));
  const out = new Float32Array(length);
  const n = channels.length;
  for (const ch of channels) {
    for (let i = 0; i < ch.length; i++) out[i] += ch[i] / n;
  }
  return out;
}

/** Linear-interpolation resampler, used when OfflineAudioContext is unavailable. */
export function resampleLinear(samples: Float32Array, fromRate: number, toRate: number): Float32Array {
  if (fromRate === toRate || samples.length === 0) return new Float32Array(samples);
  const outLength = Math.max(1, Math.round((samples.length * toRate) / fromRate));
  const out = new Float32Array(outLength);
  const ratio = fromRate / toRate;
  for (let i = 0; i < outLength; i++) {
    const pos = i * ratio;
    const i0 = Math.floor(pos);
    const i1 = Math.min(i0 + 1, samples.length - 1);
    const frac = pos - i0;
    out[i] = samples[Math.min(i0, samples.length - 1)] * (1 - frac) + samples[i1] * frac;
  }
  return out;
}

export type DecodeStage = "fetching" | "decoding" | "resampling" | "uploading";

type AudioContextCtor = typeof AudioContext;

function getAudioContextCtor(): AudioContextCtor | null {
  const w = window as unknown as { AudioContext?: AudioContextCtor; webkitAudioContext?: AudioContextCtor };
  return w.AudioContext ?? w.webkitAudioContext ?? null;
}

/**
 * Fetch `url`, decode it with Web Audio, downmix to mono and resample to 16 kHz.
 * Throws a readable Error when the webview cannot decode the format.
 */
export async function decodeForTranscription(
  url: string,
  onStage: (stage: DecodeStage) => void = () => {},
): Promise<Float32Array> {
  onStage("fetching");
  const res = await fetch(url);
  if (!res.ok) throw new Error(`Could not read the file (HTTP ${res.status}).`);
  return decodeBytesForTranscription(await res.arrayBuffer(), onStage);
}

/** Decode file bytes with Web Audio, downmix to mono and resample to 16 kHz. */
export async function decodeBytesForTranscription(
  bytes: ArrayBuffer,
  onStage: (stage: DecodeStage) => void = () => {},
): Promise<Float32Array> {
  onStage("decoding");
  const Ctor = getAudioContextCtor();
  if (!Ctor) throw new Error("This webview has no Web Audio support.");
  const ctx = new Ctor();
  let decoded: AudioBuffer;
  try {
    decoded = await ctx.decodeAudioData(bytes);
  } catch {
    throw new Error("The webview could not decode this file either. Try converting it to MP3 or WAV.");
  } finally {
    void ctx.close().catch(() => {});
  }

  onStage("resampling");
  const channels: Float32Array[] = [];
  for (let c = 0; c < decoded.numberOfChannels; c++) channels.push(decoded.getChannelData(c));
  const mono = downmixToMono(channels);
  return resampleTo16k(mono, decoded.sampleRate);
}

export async function resampleTo16k(mono: Float32Array, fromRate: number): Promise<Float32Array> {
  const target = TRANSCRIPTION_SAMPLE_RATE;
  if (fromRate === target) return mono;
  if (typeof OfflineAudioContext === "undefined") return resampleLinear(mono, fromRate, target);
  const length = Math.max(1, Math.ceil((mono.length * target) / fromRate));
  const offline = new OfflineAudioContext(1, length, target);
  const buffer = offline.createBuffer(1, mono.length, fromRate);
  buffer.copyToChannel(mono as Float32Array<ArrayBuffer>, 0);
  const src = offline.createBufferSource();
  src.buffer = buffer;
  src.connect(offline.destination);
  src.start();
  const rendered = await offline.startRendering();
  return new Float32Array(rendered.getChannelData(0));
}
