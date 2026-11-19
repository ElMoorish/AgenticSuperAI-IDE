/** Pictures attached to a prompt: validated, downscaled when large, and held as base64. */

export const MAX_IMAGES = 4;
/** What the host accepts per image (base64 characters ~ 7 MB = about 5 MB of picture). */
export const MAX_IMAGE_BYTES = 5_000_000;
/** Longest edge sent to the model: more pixels cost time and context without helping most screenshots. */
export const MAX_EDGE = 1568;
const RESIZE_ABOVE_BYTES = 1_500_000;

export const IMAGE_TYPES = ["image/png", "image/jpeg", "image/webp", "image/gif"];

export interface Attachment {
  id: number;
  mime: string;
  /** Base64 without the `data:` prefix: what the host takes. */
  data: string;
  /** `data:` URL for the thumbnail. */
  url: string;
  name: string;
}

let nextAttachmentId = 1;

const toDataUrl = (blob: Blob) =>
  new Promise<string>((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(String(r.result));
    r.onerror = () => reject(r.error ?? new Error("cannot read the image"));
    r.readAsDataURL(blob);
  });

/** Scale a big picture down (JPEG); returns null when the platform cannot (or need not) do it. */
async function shrink(file: Blob): Promise<Blob | null> {
  try {
    if (typeof createImageBitmap !== "function" || typeof document === "undefined") return null;
    const bmp = await createImageBitmap(file);
    const edge = Math.max(bmp.width, bmp.height);
    if (edge <= MAX_EDGE && file.size <= RESIZE_ABOVE_BYTES) {
      bmp.close();
      return null;
    }
    const k = Math.min(1, MAX_EDGE / edge);
    const canvas = document.createElement("canvas");
    canvas.width = Math.max(1, Math.round(bmp.width * k));
    canvas.height = Math.max(1, Math.round(bmp.height * k));
    const ctx = canvas.getContext("2d");
    if (!ctx) return null;
    ctx.fillStyle = "#fff"; // JPEG has no alpha
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    ctx.drawImage(bmp, 0, 0, canvas.width, canvas.height);
    bmp.close();
    return await new Promise<Blob | null>((res) => canvas.toBlob(res, "image/jpeg", 0.88));
  } catch {
    return null;
  }
}

/** A picture from a file, paste or drop; rejects with a short message fit for the user. */
export async function prepareImage(file: Blob, name = "image"): Promise<Attachment> {
  if (!IMAGE_TYPES.includes(file.type)) throw new Error("Only PNG, JPEG, WebP or GIF pictures can be attached.");
  let blob: Blob = file;
  if (file.type !== "image/gif") {
    const smaller = await shrink(file);
    if (smaller && smaller.size < file.size) blob = smaller;
  }
  if (blob.size > MAX_IMAGE_BYTES) throw new Error("That picture is larger than 5 MB.");
  const url = await toDataUrl(blob);
  const comma = url.indexOf(",");
  return { id: nextAttachmentId++, mime: blob.type, data: url.slice(comma + 1), url, name };
}

/** Image files in a clipboard or drop payload. */
export function imageFiles(list: FileList | File[] | null | undefined): File[] {
  return Array.from(list ?? []).filter((f) => f.type.startsWith("image/"));
}
