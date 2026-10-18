import type { SealGeneratorSettings } from './types';

export type SealTemplate = 'round' | 'oval' | 'square';
export type { SealGeneratorSettings } from './types';

export interface GeneratedSealOptions extends SealGeneratorSettings {
  organization: string;
  purpose: string;
  serial: string;
}

export interface SealCleanupOptions {
  remove_background: boolean;
  red_ink_only: boolean;
  ink_texture: boolean;
}

export const sealColors = ['#d6212e', '#ef2a2d', '#b91f2b'] as const;

export const defaultSealGeneratorSettings: SealGeneratorSettings = {
  schema_version: 2,
  template: 'round',
  color: sealColors[0],
  wear: 3,
  outer_border_width: 18,
  organization_font_size: 62,
  organization_spacing: 2,
  organization_inset: 6,
  organization_height: 142,
  organization_width: 92,
  text_stroke_width: 2,
  star_size: 92,
  purpose_offset_y: 0,
  purpose_font_size: 42,
  purpose_spacing: 4,
  serial_font_size: 26,
  serial_spacing: 0,
  serial_inset: 32,
  serial_width: 82,
};

const serifFont = '"SimSun", "STSong", "Songti SC", "Noto Serif CJK SC", serif';
const squareFont = '"STKaiti", "KaiTi", "Microsoft YaHei", serif';
const exportScale = 2;

export function drawGeneratedSeal(canvas: HTMLCanvasElement, options: GeneratedSealOptions) {
  const [width, height] = sealDimensions(options.template);
  canvas.width = width * exportScale;
  canvas.height = height * exportScale;
  const context = canvas.getContext('2d', { willReadFrequently: true });
  if (!context) throw new Error('当前浏览器无法生成印章预览');
  context.clearRect(0, 0, canvas.width, canvas.height);
  context.save();
  context.scale(exportScale, exportScale);
  context.strokeStyle = options.color;
  context.fillStyle = options.color;
  context.lineCap = 'round';
  context.lineJoin = 'round';
  context.globalAlpha = 0.98;

  if (options.template === 'round') drawRoundSeal(context, options);
  if (options.template === 'oval') drawOvalSeal(context, options);
  if (options.template === 'square') drawSquareSeal(context, options);

  context.restore();
  applyInkTexture(
    context,
    canvas.width,
    canvas.height,
    options.wear,
    seedFrom(options),
    options.template,
    exportScale,
  );
}

export async function generatedSealBlob(options: GeneratedSealOptions) {
  const canvas = document.createElement('canvas');
  drawGeneratedSeal(canvas, options);
  return canvasBlob(canvas);
}

export async function cleanupSealPreview(file: File, options: SealCleanupOptions) {
  const bitmap = await createImageBitmap(file);
  try {
    if (bitmap.width < 32 || bitmap.height < 32 || bitmap.width > 4096 || bitmap.height > 4096) {
      throw new Error('印章图片尺寸必须在 32 到 4096 像素之间');
    }
    const canvas = document.createElement('canvas');
    canvas.width = bitmap.width;
    canvas.height = bitmap.height;
    const context = canvas.getContext('2d', { willReadFrequently: true });
    if (!context) throw new Error('当前浏览器无法处理印章图片');
    context.drawImage(bitmap, 0, 0);
    processUploadedPixels(context, bitmap.width, bitmap.height, options);
    return canvasBlob(canvas);
  } finally {
    bitmap.close();
  }
}

function sealDimensions(template: SealTemplate): [number, number] {
  if (template === 'oval') return [840, 560];
  if (template === 'square') return [640, 640];
  return [720, 720];
}

export function sealOutputDimensions(template: SealTemplate): [number, number] {
  const [width, height] = sealDimensions(template);
  return [width * exportScale, height * exportScale];
}

function drawRoundSeal(context: CanvasRenderingContext2D, options: GeneratedSealOptions) {
  const center = 360;
  const outerRadius = 314;
  context.lineWidth = options.outer_border_width;
  context.beginPath();
  context.arc(center, center, outerRadius, 0, Math.PI * 2);
  context.stroke();

  const organization = cleanText(options.organization, 30);
  if (organization) {
    const length = Array.from(organization).length;
    const fontSize = clamp(Math.min(options.organization_font_size, 760 / length), 34, 68);
    const textScaleX = options.organization_width / 100;
    const textScaleY = options.organization_height / 100;
    const textRadius = clamp(
      outerRadius
        - options.outer_border_width / 2
        - options.organization_inset
        - fontSize * textScaleY * 0.5,
      205,
      286,
    );
    drawMeasuredEllipseText(
      context,
      organization,
      center,
      center,
      textRadius,
      textRadius,
      -Math.PI / 2,
      1,
      fontSize,
      options.organization_spacing,
      serifFont,
      textScaleX,
      textScaleY,
      600,
      options.text_stroke_width,
    );
  }

  drawStar(context, center, 354, options.star_size, options.star_size * 0.39);

  const purpose = cleanText(options.purpose, 14);
  if (purpose) {
    drawFittedText(
      context,
      purpose,
      center,
      506 + options.purpose_offset_y,
      390,
      options.purpose_font_size,
      options.purpose_spacing,
      serifFont,
      600,
      1,
      options.text_stroke_width,
    );
  }

  const serial = cleanText(options.serial, 32);
  if (serial) {
    const fontSize = clamp(
      Math.min(options.serial_font_size, 390 / Array.from(serial).length),
      18,
      36,
    );
    const serialRadius = clamp(outerRadius - options.serial_inset, 244, 296);
    drawMeasuredEllipseText(
      context,
      serial,
      center,
      center,
      serialRadius,
      serialRadius,
      Math.PI / 2,
      -1,
      fontSize,
      options.serial_spacing,
      serifFont,
      options.serial_width / 100,
      1,
      600,
      Math.max(0, options.text_stroke_width - 1),
    );
  }
}

function drawOvalSeal(context: CanvasRenderingContext2D, options: GeneratedSealOptions) {
  const centerX = 420;
  const centerY = 280;
  const outerRadiusX = 378;
  const outerRadiusY = 230;
  context.lineWidth = options.outer_border_width;
  context.beginPath();
  context.ellipse(centerX, centerY, outerRadiusX, outerRadiusY, 0, 0, Math.PI * 2);
  context.stroke();

  const organization = cleanText(options.organization, 30);
  if (organization) {
    const length = Array.from(organization).length;
    const fontSize = clamp(
      Math.min(options.organization_font_size, 820 / length),
      24,
      64,
    );
    const textScaleX = options.organization_width / 100;
    const textScaleY = options.organization_height / 125;
    const glyphHalfHeight = fontSize * textScaleY * 0.5;
    const textRadiusX = clamp(
      outerRadiusX
        - options.outer_border_width / 2
        - options.organization_inset
        - glyphHalfHeight,
      280,
      344,
    );
    const textRadiusY = clamp(
      outerRadiusY
        - options.outer_border_width / 2
        - options.organization_inset
        - glyphHalfHeight,
      140,
      191,
    );
    drawMeasuredEllipseText(
      context,
      organization,
      centerX,
      centerY,
      textRadiusX,
      textRadiusY,
      -Math.PI / 2,
      1,
      fontSize,
      options.organization_spacing,
      serifFont,
      textScaleX,
      textScaleY,
      600,
      options.text_stroke_width,
    );
  }

  const serial = cleanText(options.serial, 32);
  if (serial) {
    const serialY = 302 + (30 - options.serial_inset) * 1.2;
    drawFittedText(
      context,
      serial,
      centerX,
      serialY,
      500,
      options.serial_font_size,
      options.serial_spacing,
      serifFont,
      600,
      options.serial_width / 100,
      Math.max(0, options.text_stroke_width - 1),
    );
  }

  const purpose = cleanText(options.purpose, 14);
  if (purpose) {
    drawFittedText(
      context,
      purpose,
      centerX,
      396 + options.purpose_offset_y,
      510,
      options.purpose_font_size,
      options.purpose_spacing,
      serifFont,
      600,
      1,
      options.text_stroke_width,
    );
  }
}

function drawSquareSeal(context: CanvasRenderingContext2D, options: GeneratedSealOptions) {
  context.lineWidth = options.outer_border_width;
  context.strokeRect(48, 48, 544, 544);
  context.lineWidth = 5;
  context.strokeRect(68, 68, 504, 504);

  const combined = cleanText(`${options.organization}${options.purpose}`, 6);
  const characters = Array.from(combined);
  if (characters.length) {
    const rows = characters.length <= 4 ? 2 : 3;
    const columns = Math.ceil(characters.length / rows);
    const fontSize = rows === 2 ? 190 : 126;
    const columnGap = columns === 1 ? 0 : 218;
    const firstX = columns === 1 ? 320 : 430;
    const top = rows === 2 ? 185 : 140;
    const rowGap = rows === 2 ? 205 : 145;
    context.textAlign = 'center';
    context.textBaseline = 'middle';
    context.font = `900 ${fontSize}px ${squareFont}`;
    characters.forEach((character, index) => {
      const column = Math.floor(index / rows);
      const row = index % rows;
      context.fillText(character, firstX - column * columnGap, top + row * rowGap);
    });
  }

  const serial = cleanText(options.serial, 32);
  if (serial) drawFittedText(context, serial, 320, 536, 430, 38, 1, serifFont);
}

function drawMeasuredEllipseText(
  context: CanvasRenderingContext2D,
  text: string,
  centerX: number,
  centerY: number,
  radiusX: number,
  radiusY: number,
  centerAngle: number,
  direction: 1 | -1,
  fontSize: number,
  tracking: number,
  fontFamily: string,
  scaleX = 1,
  scaleY = 1,
  fontWeight = 600,
  strokeWidth = 0,
) {
  const characters = Array.from(text);
  if (!characters.length) return;
  context.font = `${fontWeight} ${fontSize}px ${fontFamily}`;
  context.textAlign = 'center';
  context.textBaseline = 'middle';
  const widths = characters.map((character) => context.measureText(character).width * scaleX);
  const totalWidth = widths.reduce((total, width) => total + width, 0)
    + Math.max(0, characters.length - 1) * tracking;
  const centerTangentRadius = Math.max(
    1,
    Math.hypot(radiusX * Math.sin(centerAngle), radiusY * Math.cos(centerAngle)),
  );
  let cursor = 0;
  characters.forEach((character, index) => {
    const offset = cursor + widths[index] / 2 - totalWidth / 2;
    const angle = centerAngle + direction * offset / centerTangentRadius;
    const tangentX = -radiusX * Math.sin(angle) * direction;
    const tangentY = radiusY * Math.cos(angle) * direction;
    context.save();
    context.translate(centerX + radiusX * Math.cos(angle), centerY + radiusY * Math.sin(angle));
    context.rotate(Math.atan2(tangentY, tangentX));
    context.scale(scaleX, scaleY);
    drawInkGlyph(context, character, strokeWidth);
    context.restore();
    cursor += widths[index] + tracking;
  });
}

function drawFittedText(
  context: CanvasRenderingContext2D,
  text: string,
  centerX: number,
  centerY: number,
  maxWidth: number,
  preferredSize: number,
  spacing: number,
  fontFamily: string,
  fontWeight = 600,
  scaleX = 1,
  strokeWidth = 0,
) {
  const characters = Array.from(text);
  let fontSize = preferredSize;
  context.textAlign = 'center';
  context.textBaseline = 'middle';
  while (fontSize > 18) {
    context.font = `${fontWeight} ${fontSize}px ${fontFamily}`;
    const width = characters.reduce((total, character) => total + context.measureText(character).width * scaleX, 0)
      + Math.max(0, characters.length - 1) * spacing;
    if (width <= maxWidth) break;
    fontSize -= 2;
  }
  context.font = `${fontWeight} ${fontSize}px ${fontFamily}`;
  const widths = characters.map((character) => context.measureText(character).width * scaleX);
  const totalWidth = widths.reduce((total, width) => total + width, 0) + Math.max(0, characters.length - 1) * spacing;
  let x = centerX - totalWidth / 2;
  characters.forEach((character, index) => {
    context.save();
    context.translate(x + widths[index] / 2, centerY);
    context.scale(scaleX, 1);
    drawInkGlyph(context, character, strokeWidth);
    context.restore();
    x += widths[index] + spacing;
  });
}

function drawInkGlyph(context: CanvasRenderingContext2D, character: string, strokeWidth: number) {
  if (strokeWidth > 0) {
    context.lineWidth = strokeWidth;
    context.strokeText(character, 0, 0);
  }
  context.fillText(character, 0, 0);
}

function drawStar(context: CanvasRenderingContext2D, centerX: number, centerY: number, outerRadius: number, innerRadius: number) {
  context.beginPath();
  for (let index = 0; index < 10; index += 1) {
    const radius = index % 2 === 0 ? outerRadius : innerRadius;
    const angle = -Math.PI / 2 + index * Math.PI / 5;
    const x = centerX + Math.cos(angle) * radius;
    const y = centerY + Math.sin(angle) * radius;
    if (index === 0) context.moveTo(x, y);
    else context.lineTo(x, y);
  }
  context.closePath();
  context.fill();
}

function applyInkTexture(
  context: CanvasRenderingContext2D,
  width: number,
  height: number,
  wear: number,
  seed: number,
  template: SealTemplate,
  scale: number,
) {
  const strength = clamp(wear, 0, 30);
  if (strength <= 0) return;
  eraseInkPatches(context, width, height, strength, seed, template, scale);
  const image = context.getImageData(0, 0, width, height);
  for (let offset = 3; offset < image.data.length; offset += 4) {
    if (image.data[offset] === 0) continue;
    const pixel = (offset - 3) / 4;
    const x = (pixel % width) / scale;
    const y = Math.floor(pixel / width) / scale;
    const coarse = textureHash(Math.floor(x / 8), Math.floor(y / 8), seed);
    const fine = textureHash(x, y, seed ^ 0x9e3779b9);
    const pressure = 0.94 + ((textureHash(Math.floor(x / 34), Math.floor(y / 26), seed ^ 0x52dce729) & 255) / 255) * 0.06;
    const fade = 1 - strength * 0.0012 - ((coarse & 255) / 255) * strength * 0.002;
    const pinhole = fine % 7000 < strength * 1.8 ? 0.18 : 1;
    image.data[offset] = Math.round(image.data[offset] * pressure * fade * pinhole);
  }
  context.putImageData(image, 0, 0);
}

function eraseInkPatches(
  context: CanvasRenderingContext2D,
  width: number,
  height: number,
  strength: number,
  seed: number,
  template: SealTemplate,
  scale: number,
) {
  const source = context.getImageData(0, 0, width, height);
  const candidates: Array<[number, number]> = [];
  const sampleStep = Math.max(3, Math.round(5 * scale));
  for (let y = Math.round(2 * scale); y < height; y += sampleStep) {
    for (let x = Math.round(2 * scale); x < width; x += sampleStep) {
      if (source.data[(y * width + x) * 4 + 3] > 72) candidates.push([x, y]);
    }
  }
  if (!candidates.length) return;

  const random = seededRandom(seed ^ 0xa511e9b3);
  const intensity = strength / 30;
  const patchCount = Math.max(3, Math.round(strength * 1.15));
  context.save();
  context.globalCompositeOperation = 'destination-out';
  context.fillStyle = '#000';
  for (let index = 0; index < patchCount; index += 1) {
    const [x, y] = candidates[Math.floor(random() * candidates.length)];
    const radiusX = (2.5 + random() * (5 + intensity * 9)) * scale;
    const radiusY = (1.5 + random() * (3 + intensity * 6)) * scale;
    context.save();
    context.translate(x, y);
    context.rotate((random() - 0.5) * Math.PI);
    context.globalAlpha = 0.28 + random() * (0.32 + intensity * 0.28);
    context.beginPath();
    context.ellipse(0, 0, radiusX, radiusY, 0, 0, Math.PI * 2);
    context.fill();
    context.restore();
  }

  const streakCount = Math.max(1, Math.floor(strength / 8));
  context.lineCap = 'round';
  for (let index = 0; index < streakCount; index += 1) {
    const [x, y] = candidates[Math.floor(random() * candidates.length)];
    const length = (8 + random() * (14 + intensity * 20)) * scale;
    const angle = random() * Math.PI;
    context.globalAlpha = 0.22 + random() * 0.3;
    context.lineWidth = (1.2 + random() * (1.5 + intensity * 2.5)) * scale;
    context.beginPath();
    context.moveTo(x - Math.cos(angle) * length / 2, y - Math.sin(angle) * length / 2);
    context.lineTo(x + Math.cos(angle) * length / 2, y + Math.sin(angle) * length / 2);
    context.stroke();
  }

  context.globalAlpha = 0.72;
  context.lineWidth = (2.2 + intensity * 3.5) * scale;
  if (template === 'round') {
    const breakCount = Math.max(1, Math.round(strength / 7));
    for (let index = 0; index < breakCount; index += 1) {
      const angle = random() * Math.PI * 2;
      const span = 0.012 + random() * (0.016 + intensity * 0.018);
      context.beginPath();
      context.arc(width / 2, height / 2, 314 * scale, angle - span / 2, angle + span / 2);
      context.stroke();
    }
  } else if (template === 'oval') {
    const angle = random() * Math.PI * 2;
    const span = 0.014 + random() * 0.025;
    context.beginPath();
    context.ellipse(
      width / 2,
      height / 2,
      378 * scale,
      230 * scale,
      0,
      angle - span / 2,
      angle + span / 2,
    );
    context.stroke();
  }
  context.restore();
}

function seededRandom(seed: number) {
  let state = seed >>> 0;
  return () => {
    state += 0x6d2b79f5;
    let value = state;
    value = Math.imul(value ^ (value >>> 15), value | 1);
    value ^= value + Math.imul(value ^ (value >>> 7), value | 61);
    return ((value ^ (value >>> 14)) >>> 0) / 4294967296;
  };
}

function processUploadedPixels(
  context: CanvasRenderingContext2D,
  width: number,
  height: number,
  options: SealCleanupOptions,
) {
  if (!options.remove_background && !options.red_ink_only && !options.ink_texture) return;
  const image = context.getImageData(0, 0, width, height);
  for (let offset = 0; offset < image.data.length; offset += 4) {
    const originalAlpha = image.data[offset + 3];
    if (!originalAlpha) continue;
    if (options.remove_background || options.red_ink_only) {
      const minimum = Math.min(image.data[offset], image.data[offset + 1], image.data[offset + 2]);
      const coverage = (255 - minimum) / 255;
      if (coverage <= 0.02) {
        image.data[offset + 3] = 0;
        continue;
      }
      const corrected = [
        removeWhiteMatte(image.data[offset], coverage),
        removeWhiteMatte(image.data[offset + 1], coverage),
        removeWhiteMatte(image.data[offset + 2], coverage),
      ];
      const redStrength = options.red_ink_only ? redInkStrength(corrected) : 1;
      if (redStrength <= 0) {
        image.data[offset + 3] = 0;
        continue;
      }
      image.data[offset] = corrected[0];
      image.data[offset + 1] = corrected[1];
      image.data[offset + 2] = corrected[2];
      image.data[offset + 3] = Math.round(originalAlpha * coverage * redStrength);
    }
    if (options.ink_texture && image.data[offset + 3]) {
      const pixel = offset / 4;
      const x = pixel % width;
      const y = Math.floor(pixel / width);
      const coarse = textureHash(Math.floor(x / 7), Math.floor(y / 7), width);
      const fine = textureHash(x, y, width ^ 0x9e3779b9);
      const variation = 0.91 + ((coarse & 255) / 255) * 0.08;
      const pinhole = fine % 5000 < 8 ? 0.42 : 1;
      image.data[offset + 3] = Math.round(image.data[offset + 3] * variation * pinhole);
    }
  }
  context.putImageData(image, 0, 0);
}

function removeWhiteMatte(channel: number, coverage: number) {
  return clamp(Math.round((channel - 255 * (1 - coverage)) / coverage), 0, 255);
}

function redInkStrength(color: number[]) {
  const red = color[0];
  const other = Math.max(color[1], color[2]);
  const minimum = Math.min(color[0], color[1], color[2]);
  if (red < 55 || red - other <= 10) return 0;
  const dominance = clamp((red - other - 10) / 70, 0, 1);
  const saturation = clamp((Math.max(red, other) - minimum - 12) / 60, 0, 1);
  return Math.min(dominance, saturation);
}

function cleanText(value: string, maxLength: number) {
  return Array.from(value.trim().replace(/\s+/g, '')).slice(0, maxLength).join('');
}

function seedFrom(options: GeneratedSealOptions) {
  let value = 2166136261;
  const source = [
    options.template,
    options.organization,
    options.purpose,
    options.serial,
    options.outer_border_width,
    options.organization_font_size,
    options.organization_spacing,
    options.organization_inset,
    options.organization_height,
    options.organization_width,
    options.text_stroke_width,
    options.star_size,
    options.purpose_offset_y,
    options.purpose_font_size,
    options.purpose_spacing,
    options.serial_font_size,
    options.serial_spacing,
    options.serial_inset,
    options.serial_width,
  ].join('|');
  for (const character of source) {
    value ^= character.codePointAt(0) || 0;
    value = Math.imul(value, 16777619);
  }
  return value >>> 0;
}

function textureHash(x: number, y: number, seed: number) {
  let value = (Math.imul(x, 0x45d9f3b) + Math.imul(y, 0x119de1f3) + seed) >>> 0;
  value ^= value >>> 16;
  value = Math.imul(value, 0x45d9f3b) >>> 0;
  return (value ^ (value >>> 16)) >>> 0;
}

function canvasBlob(canvas: HTMLCanvasElement) {
  return new Promise<Blob>((resolve, reject) => {
    canvas.toBlob((blob) => {
      if (blob) resolve(blob);
      else reject(new Error('生成印章图片失败'));
    }, 'image/png');
  });
}

function clamp(value: number, minimum: number, maximum: number) {
  return Math.min(maximum, Math.max(minimum, value));
}
