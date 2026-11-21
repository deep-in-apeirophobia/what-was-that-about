// Files for Ask messages: which ones can be attached, and shrinking images that are too
// big (or in a format vision models don't take) before they're sent.

export type AttachmentKind = "image" | "audio";

export const IMAGE_EXTENSIONS = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "avif", "heic", "heif", "tif", "tiff"];
export const VOICE_EXTENSIONS = [
  "mp3", "m4a", "aac", "wav", "flac", "ogg", "opus", "oga", "webm",
  "mp4", "mkv", "mov", "wma", "amr", "3gp", "aif", "aiff", "caf",
];

/** Attachments per message (the backend enforces the same limit). */
export const MAX_ATTACHMENTS = 8;
/** Longest voice message, in seconds (the backend enforces the same limit). */
export const MAX_VOICE_SECS = 20 * 60;

/** What a file would be as an attachment, judged by its MIME type or name; null if neither. */
export function attachmentKind(name: string, mime = ""): AttachmentKind | null {
  if (mime.startsWith("image/")) return "image";
  if (mime.startsWith("audio/")) return "audio";
  const ext = name.includes(".") ? name.split(".").pop()!.toLowerCase() : "";
  if (IMAGE_EXTENSIONS.includes(ext)) return "image";
  if (VOICE_EXTENSIONS.includes(ext)) return "audio";
  return null;
}

/** Image types every vision model takes as they are. */
const DIRECT_IMAGE_TYPES = ["image/png", "image/jpeg", "image/webp", "image/gif"];
/** Bigger images are re-encoded: some providers reject images over 5 MB. */
export const MAX_DIRECT_IMAGE_BYTES = 3_500_000;
export const MAX_IMAGE_SIDE = 2048;

/** Size that fits within `max` px on the long side, never enlarging. */
export function fitWithin(width: number, height: number, max: number): { width: number; height: number } {
  const scale = Math.min(1, max / Math.max(width, height, 1));
  return { width: Math.max(1, Math.round(width * scale)), height: Math.max(1, Math.round(height * scale)) };
}

export function needsReencode(type: string, size: number): boolean {
  return !DIRECT_IMAGE_TYPES.includes(type) || size > MAX_DIRECT_IMAGE_BYTES;
}

/** Image bytes ready to send: small PNG/JPEG/WebP/GIF files unchanged, anything else redrawn
 *  as a JPEG of at most 2048 px. */
export async function prepareImage(blob: Blob): Promise<Uint8Array> {
  if (!needsReencode(blob.type, blob.size)) return new Uint8Array(await blob.arrayBuffer());
  const img = await loadImage(blob);
  const { width, height } = fitWithin(img.naturalWidth, img.naturalHeight, MAX_IMAGE_SIDE);
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("Couldn't process the image.");
  // JPEG has no transparency
  ctx.fillStyle = "#fff";
  ctx.fillRect(0, 0, width, height);
  ctx.drawImage(img, 0, 0, width, height);
  const out = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/jpeg", 0.88));
  if (!out) throw new Error("Couldn't process the image.");
  return new Uint8Array(await out.arrayBuffer());
}

function loadImage(blob: Blob): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const url = URL.createObjectURL(blob);
    const img = new Image();
    img.onload = () => {
      URL.revokeObjectURL(url);
      resolve(img);
    };
    img.onerror = () => {
      URL.revokeObjectURL(url);
      reject(new Error("This image format can't be read here. Try PNG or JPEG."));
    };
    img.src = url;
  });
}

/** "1:05" style length of a voice clip. */
export function clipLength(secs: number | null | undefined): string {
  const s = Math.max(0, Math.round(secs ?? 0));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}
