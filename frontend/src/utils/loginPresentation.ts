/* MoviePilot-Frontend v3 609ea05, MIT; see frontend/third-party/MoviePilot-LICENSE. */
import type { GlassAppearance, GlassOpticalPreset } from '@/utils/glassOptics'

export interface LoginGlassPreference {
  /** 用户选择的玻璃材质。 */
  appearance: GlassAppearance
  /** 用户保存的局部非均匀形变强度。 */
  deformationStrength: number
  /** 用户保存的轨迹、尾波与惯性强度。 */
  flowStrength: number
  /** 用户选择的方案；登录页保持方案身份但只强制高质量能力。 */
  preset: GlassOpticalPreset
  /** 用户保存的方向反射亮度。 */
  reflectionStrength: number
  /** 用户保存的玻璃内部透射亮度。 */
  transmissionStrength: number
  /** 用户保存的统一采样平移强度。 */
  translationStrength: number
  /** 用户保存的壁纸可见度与材质遮罩强度。 */
  transparencyStrength: number
}

export interface LoginBackgroundLayer {
  /** 跨登录状态保持稳定的呈现槽位。 */
  key: 'back' | 'front'
  /** 壁纸在交叉淡化中的职责；standby 槽位可提前准备下一张。 */
  role: 'active' | 'previous' | 'standby'
  /** 当前槽位显示的壁纸地址。 */
  url: string
}

/** 玻璃登录页只强制高质量能力，六个用户强度在登录前后保持一致。 */
export function getLoginGlassOpticalSettings(preference: LoginGlassPreference) {
  return {
    appearance: preference.appearance,
    deformationStrength: preference.deformationStrength,
    flowStrength: preference.flowStrength,
    preset: preference.preset,
    quality: 'high' as const,
    reflectionStrength: preference.reflectionStrength,
    transmissionStrength: preference.transmissionStrength,
    translationStrength: preference.translationStrength,
    transparencyStrength: preference.transparencyStrength,
  }
}
