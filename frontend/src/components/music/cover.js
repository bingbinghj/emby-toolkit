// 先在浏览器生成有界 JPEG，再通过既有预览流程保存；不创建服务器暂存文件。
export async function prepareMusicCover(file) {
  if (!['image/jpeg','image/png','image/webp'].includes(file.type)) throw new Error('请选择 JPEG、PNG 或 WebP 图片');
  if (file.size > 15 * 1024 * 1024) throw new Error('图片不能超过 15 MB');
  const url = URL.createObjectURL(file);
  try {
    const image = new Image();
    image.src = url;
    await image.decode();
    const scale = Math.min(1, 1600/image.naturalWidth, 1600/image.naturalHeight);
    const canvas = document.createElement('canvas');
    canvas.width = Math.max(1, Math.round(image.naturalWidth*scale));
    canvas.height = Math.max(1, Math.round(image.naturalHeight*scale));
    const context = canvas.getContext('2d');
    if (!context) throw new Error('当前浏览器无法处理图片');
    context.fillStyle = '#ffffff';
    context.fillRect(0,0,canvas.width,canvas.height);
    context.drawImage(image,0,0,canvas.width,canvas.height);
    for (const quality of [.92,.8,.65,.45]) {
      const data = canvas.toDataURL('image/jpeg',quality);
      if (data.length < 1_398_000) return data;
    }
    throw new Error('图片压缩后仍然过大，请选择较小的图片');
  } finally { URL.revokeObjectURL(url); }
}
