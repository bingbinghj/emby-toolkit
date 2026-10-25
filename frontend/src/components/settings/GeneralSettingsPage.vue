<!-- frontend/src/components/settings/GeneralSettingsPage.vue -->
<template>
  <n-layout class="mp-page settings-page" :content-style="pageContentStyle">
    <n-page-header class="mp-page-header" title="通用设置" subtitle="配置媒体处理、服务连接和通知" />
    <n-space vertical :size="pageSpaceSize" :style="pageSpaceStyle">

      <!-- ★★★ 最终修正: v-if, v-else-if, v-else 现在是正确的同级兄弟关系 ★★★ -->
      <div v-if="configModel">
        <n-form
          ref="formRef"
          :rules="formRules"
          @submit.prevent
          label-placement="top"
          label-width="auto"
          label-align="left"
          :model="configModel"
        >
          <n-tabs class="settings-tabs" type="line" animated :size="isMobile ? 'medium' : 'large'" :pane-style="tabPaneStyle">
            <!-- ================== 标签页 1: 通用设置 ================== -->
            <n-tab-pane name="general"><template #tab><span class="mp-title-with-icon"><UiIcon name="settings" :size="16" />通用设置</span></template>
              <n-grid class="mp-aligned-grid settings-card-grid" cols="1" :x-gap="24" :y-gap="24">
                <!-- 左侧列 -->
                <n-gi>
                  <n-card :bordered="false" class="dashboard-card">
                    <template #header><span class="card-title">基础设置</span></template>
                    <n-form-item-grid-item label="处理项目间的延迟 (秒)" path="delay_between_items_sec">
                      <n-input-number v-model:value="configModel.delay_between_items_sec" :min="0" :step="0.1" placeholder="例如: 0.5"/>
                    </n-form-item-grid-item>

                    <n-form-item-grid-item label="媒体信息验收等待 (秒)" path="media_info_retry_interval">
                      <n-input-number
                        v-model:value="configModel.media_info_retry_interval"
                        :min="5"
                        :max="1800"
                        :step="5"
                        placeholder="例如: 30"
                      />
                      <template #feedback>
                        <n-text depth="3" style="font-size:0.8em;">
                          每轮单项补提取后等待该时长再验收。
                        </n-text>
                      </template>
                    </n-form-item-grid-item>

                    <n-form-item-grid-item label="媒体信息验收重试次数" path="media_info_retry_attempts">
                      <n-input-number
                        v-model:value="configModel.media_info_retry_attempts"
                        :min="0"
                        :max="20"
                        :step="1"
                        placeholder="例如: 3"
                      />
                      <template #feedback>
                        <n-text depth="3" style="font-size:0.8em;">
                          仅重试仍缺少有效媒体信息的项目；设为 0 时只验收，不补提取。
                        </n-text>
                      </template>
                    </n-form-item-grid-item>

                    <n-form-item-grid-item label="中文字幕默认语言" path="subtitle_chinese_default_language">
                      <n-select
                        v-model:value="configModel.subtitle_chinese_default_language"
                        :options="subtitleChineseDefaultLanguageOptions"
                      />
                      <template #feedback>
                        <n-text depth="3" style="font-size:0.8em;">
                          仅用于媒体信息编辑器中未明确简繁的中文字幕“修正语言 (Language)”选项，方便填充标题；保留明确的 chs、cht。
                        </n-text>
                      </template>
                    </n-form-item-grid-item>

                    <n-form-item-grid-item label="需手动处理的最低评分阈值" path="min_score_for_review">
                      <n-input-number v-model:value="configModel.min_score_for_review" :min="0.0" :max="10" :step="0.1" placeholder="例如: 6.0"/>
                      <template #feedback><n-text depth="3" style="font-size:0.8em;">处理质量评分低于此值的项目将进入待复核列表。</n-text></template>
                    </n-form-item-grid-item>
                    <n-form-item-grid-item label="最大主要演员数" path="max_actors_to_process">
                      <n-input-number v-model:value="configModel.max_actors_to_process" :min="10" :step="10" placeholder="建议 30-100"/>
                      <template #feedback><n-text depth="3" style="font-size:0.8em;">处理后最终演员表数量，超过会截断，优先保留有头像演员。</n-text></template>
                    </n-form-item-grid-item>
                    <n-form-item-grid-item label="最大分集演员数" path="max_episode_actors_to_process">
                      <n-input-number v-model:value="configModel.max_episode_actors_to_process" :min="0" :step="1" placeholder="例如: 5"/>
                      <template #feedback><n-text depth="3" style="font-size:0.8em;">每集最多处理的演员数量，0代表不单独处理分集演员。</n-text></template>
                    </n-form-item-grid-item>
                    <n-form-item-grid-item label="截取视频图片" path="extract_thumb">
                      <n-switch v-model:value="configModel.extract_thumb" />
                      <template #feedback>
                        <n-text depth="3" style="font-size:0.8em;">
                          从视频流中截取高清画面作为分集图或背景图。
                        </n-text>
                      </template>
                    </n-form-item-grid-item>
                    <n-form-item-grid-item label="为角色名添加前缀" path="actor_role_add_prefix">
                      <n-switch v-model:value="configModel.actor_role_add_prefix" />
                      <template #feedback><n-text depth="3" style="font-size:0.8em;">角色名前加上“饰 ”或“配 ”。</n-text></template>
                    </n-form-item-grid-item>
                    <n-form-item-grid-item label="移除无头像的演员" path="remove_actors_without_avatars">
                      <n-switch v-model:value="configModel.remove_actors_without_avatars" />
                      <template #feedback>
                        <n-text depth="3" style="font-size:0.8em;">
                          在最终演员表移除那些找不到任何可用头像的演员。
                        </n-text>
                      </template>
                    </n-form-item-grid-item>
                    <n-form-item-grid-item label="关键词写入标签" path="keyword_to_tags">
                      <n-switch v-model:value="configModel.keyword_to_tags" />
                      <template #feedback>
                        <n-text depth="3" style="font-size:0.8em;">
                          将映射后的中文关键词写入标签。
                        </n-text>
                      </template>
                    </n-form-item-grid-item>
                    <n-form-item-grid-item label="工作室中文化" path="studio_to_chinese">
                      <n-switch v-model:value="configModel.studio_to_chinese" />
                      <template #feedback>
                        <n-text depth="3" style="font-size:0.8em;">
                          将工作室名称转换为中文。
                        </n-text>
                      </template>
                    </n-form-item-grid-item>
                    <template #footer>
                      <n-space justify="end">
                        <n-button type="primary" :loading="savingCard === 'basic'" :disabled="savingConfig" aria-label="保存基础设置" @click="saveCard('basic')">
                          <template #icon><n-icon :component="SaveIcon" /></template>
                          保存
                        </n-button>
                      </n-space>
                    </template>
                  </n-card>
                </n-gi>
                <!-- 第二列：实时监控 -->
                <n-gi>
                  <n-card :bordered="false" class="dashboard-card">
                    <template #header>
                      <div style="display: flex; align-items: center; gap: 8px;">
                        <span class="card-title">实时监控</span>
                      </div>
                    </template>

                    <n-form-item label="启用文件系统监控" path="monitor_enabled">
                      <n-switch v-model:value="configModel.monitor_enabled">
                        <template #checked>开启</template>
                        <template #unchecked>关闭</template>
                      </n-switch>
                    </n-form-item>

                    <n-form-item label="监控路径" path="monitor_paths">
                      <n-input-group>
                        <n-select
                          v-model:value="configModel.monitor_paths"
                          multiple
                          filterable
                          tag
                          :show-arrow="false"
                          placeholder="输入路径并回车，或点击右侧选择"
                          :options="[]"
                          style="flex: 1;"
                        />
                        <n-button type="primary" ghost aria-label="选择监控路径" @click="openLocalFolderSelector('monitor_paths', true)">
                          <template #icon><n-icon :component="FolderIcon" /></template>
                        </n-button>
                      </n-input-group>
                      <template #feedback>
                        <n-text depth="3" style="font-size:0.8em;">
                          请保持和 Emby 媒体库路径映射一致。
                        </n-text>
                      </template>
                    </n-form-item>

                    <n-form-item label="排除路径" path="monitor_exclude_dirs">
                      <n-input-group>
                        <n-select
                          v-model:value="configModel.monitor_exclude_dirs"
                          multiple
                          filterable
                          tag
                          :show-arrow="false"
                          placeholder="输入路径并回车，或点击右侧选择"
                          :options="[]"
                          style="flex: 1;"
                        />
                        <n-button type="primary" ghost aria-label="选择排除路径" @click="openLocalFolderSelector('monitor_exclude_dirs', true)">
                          <template #icon><n-icon :component="FolderIcon" /></template>
                        </n-button>
                      </n-input-group>
                      <template #feedback>
                        <n-text depth="3" style="font-size:0.8em;">
                          命中这些路径的文件将<b>跳过刮削流程</b>，仅刷新。<br/>
                        </n-text>
                      </template>
                    </n-form-item>

                    <!-- 排除刷新延迟 -->
                    <n-form-item label="排除刷新延迟" path="monitor_exclude_refresh_delay">
                      <n-input-number
                        v-model:value="configModel.monitor_exclude_refresh_delay"
                        :min="0"
                        :step="10"
                        placeholder="0"
                        style="width: 100%"
                      >
                        <template #suffix>秒</template>
                      </n-input-number>
                      <template #feedback>
                        <n-text depth="3" style="font-size:0.8em;">
                          仅对<b>排除路径</b>生效。设为 0 则立即刷新。<br/>
                        </n-text>
                      </template>
                    </n-form-item>

                    <n-form-item label="监控扩展名" path="monitor_extensions">
                      <n-select
                        v-model:value="configModel.monitor_extensions"
                        multiple
                        filterable
                        tag
                        placeholder="输入扩展名并回车"
                        :options="[]"
                      />
                      <!-- 注意：options 设为空数组配合 tag 模式允许用户自由输入 -->
                      <template #feedback>
                        <n-text depth="3" style="font-size:0.8em;">
                          仅处理这些后缀的文件，输入扩展名并回车添加新的监控文件类型。
                        </n-text>
                      </template>
                    </n-form-item>
                    <n-form-item label="图片语言偏好" path="tmdb_image_language_preference">
                      <n-radio-group v-model:value="configModel.tmdb_image_language_preference" name="image_lang_group">
                        <n-space>
                          <n-radio value="zh">简体中文优先</n-radio>
                          <n-radio value="original">原语言优先</n-radio>
                        </n-space>
                      </n-radio-group>
                      <template #feedback>
                        <n-text depth="3" style="font-size:0.8em;">
                          控制下载 海报 等图片时的语言优先级。
                        </n-text>
                      </template>
                    </n-form-item>
                    <n-form-item label="SHA1检测" path="monitor_sha1_pc_search">
                      <n-switch v-model:value="configModel.monitor_sha1_pc_search" />
                      <template #feedback>
                        <n-text depth="3" style="font-size:0.8em;">
                          文件SHA1查询，非115网盘勿启用。
                        </n-text>
                      </template>
                    </n-form-item>
                    <template #footer>
                      <n-space justify="end">
                        <n-button type="primary" :loading="savingCard === 'monitor'" :disabled="savingConfig" aria-label="保存实时监控" @click="saveCard('monitor')">
                          <template #icon><n-icon :component="SaveIcon" /></template>
                          保存
                        </n-button>
                      </n-space>
                    </template>
                  </n-card>
                </n-gi>
                <n-gi>
                  <n-card :bordered="false" class="dashboard-card">
                    <template #header><span class="card-title">数据源与API</span></template>
                    <n-form-item label="TMDB API Key" path="tmdb_api_key">
                      <n-input type="password" show-password-on="mousedown" v-model:value="configModel.tmdb_api_key" placeholder="输入你的 TMDB API Key" />
                    </n-form-item>
                    <n-form-item label="TMDB API Base URL" path="tmdb_api_base_url">
                      <n-input v-model:value="configModel.tmdb_api_base_url" placeholder="https://api.themoviedb.org/3" />
                      <template #feedback><n-text depth="3" style="font-size:0.8em;">TMDb API的基础URL，通常不需要修改。</n-text></template>
                    </n-form-item>
                    <n-form-item label="启用豆瓣辅助" path="douban_enable_online_api">
                      <n-space align="center">
                        <n-switch v-model:value="configModel.douban_enable_online_api" />
                        <n-text depth="3" style="font-size: 0.9em; margin-left: 8px;">
                          从豆瓣补充缺失的演员以及部分中文角色名。
                        </n-text>
                      </n-space>
                    </n-form-item>
                    <n-form-item-grid-item label="豆瓣API冷却时间 (秒)" path="api_douban_default_cooldown_seconds">
                      <n-input-number v-model:value="configModel.api_douban_default_cooldown_seconds" :min="0.1" :step="0.1" placeholder="例如: 1.0"/>
                    </n-form-item-grid-item>
                    <n-form-item label="豆瓣登录 Cookie" path="douban_cookie">
                      <n-input type="password" show-password-on="mousedown" v-model:value="configModel.douban_cookie" placeholder="从浏览器开发者工具中获取"/>
                      <template #feedback><n-text depth="3" style="font-size:0.8em;">非必要不用配置，作用有限。</n-text></template>
                    </n-form-item>
                    <template #footer>
                      <n-space justify="end">
                        <n-button type="primary" :loading="savingCard === 'sources'" :disabled="savingConfig" aria-label="保存数据源与API" @click="saveCard('sources')">
                          <template #icon><n-icon :component="SaveIcon" /></template>
                          保存
                        </n-button>
                      </n-space>
                    </template>
                  </n-card>
                </n-gi>
              </n-grid>
            </n-tab-pane>

            <!-- ★★★ 115 网盘设置 ★★★ -->
            <n-tab-pane name="115_drive"><template #tab><span class="mp-title-with-icon"><UiIcon name="key" :size="16" />115 网盘</span></template>
              <n-grid class="mp-aligned-grid settings-card-grid" cols="1" :x-gap="24" :y-gap="24">

                <n-gi>
                  <n-card :bordered="false" class="dashboard-card" style="height: 100%;">
                    <template #header>
                      <div style="display: flex; align-items: center; gap: 8px;">
                        <span class="card-title">账户与连接</span>
                        <n-tag v-if="p115Info" :type="p115Info.valid ? 'success' : 'error'" size="small" round>
                          {{ p115Info.msg || (p115Info.valid ? '连接正常' : '连接异常') }}
                        </n-tag>
                        <n-tag v-else type="warning" size="small" round>未检查</n-tag>
                      </div>
                    </template>
                    <template #header-extra>
                      <n-space align="center" :size="12">
                        <n-button size="small" secondary type="success" @click="check115Status(false)" :loading="loading115Info"><template #icon><UiIcon name="refresh" /></template>
                          刷新状态
                        </n-button>
                      </n-space>
                    </template>

                    <div v-if="p115Info && p115Info.user_info" style="margin-bottom: 16px; padding: 12px; background: var(--n-action-color); border-radius: 8px; display: flex; align-items: center; gap: 12px;">
                      <n-avatar :src="p115Info.user_info.user_face_m" round size="large" />
                      <div style="flex: 1; overflow: hidden;">
                        <div style="font-weight: bold; font-size: 14px; display: flex; align-items: center; gap: 6px;">
                          {{ p115Info.user_info.user_name }}
                          <n-tag size="tiny" type="warning" :bordered="false">{{ p115Info.user_info.vip_info?.level_name || '普通用户' }}</n-tag>
                        </div>
                        <div style="font-size: 12px; color: var(--n-text-color-3); margin-top: 4px;">
                          剩余空间: {{ p115Info.user_info.rt_space_info?.all_remain?.size_format || '未知' }}
                        </div>
                      </div>
                    </div>

                    <n-form-item label="授权方式" path="p115_auth_method">
                      <n-radio-group v-model:value="configModel.p115_auth_method" name="auth_method_group">
                        <n-space>
                          <n-radio value="web">网页登录授权</n-radio>
                          <n-radio value="qrcode">自定义 AppID 扫码</n-radio>
                        </n-space>
                      </n-radio-group>
                    </n-form-item>

                    <!-- 方式一：网页登录授权 -->
                    <n-form-item label="OpenAPI" v-if="configModel.p115_auth_method === 'web' || !configModel.p115_auth_method">
                      <n-space vertical :size="8" style="width: 100%;">
                        <n-space align="center" justify="space-between">
                          <n-tag :type="p115Info?.has_token ? 'success' : 'default'" size="small">
                            <template #icon>
                              <n-icon :component="p115Info?.has_token ? CheckIcon : CloseIcon" />
                            </template>
                            {{ p115Info?.has_token ? `${openApiAuthLabel}` : `未授权：${openApiAuthLabel}` }}
                          </n-tag>
                          <n-button size="small" type="warning" @click="startWebAuth" :loading="isWebAuthing"><template #icon><UiIcon name="key" /></template>
                            {{ p115Info?.has_token ? '重新登录' : '登录授权' }}
                          </n-button>
                        </n-space>
                      </n-space>
                    </n-form-item>

                    <!-- 方式二：自定义 AppID 扫码 -->
                    <n-form-item label="OpenAPI" path="p115_app_id" v-if="configModel.p115_auth_method === 'qrcode'">
                      <n-space vertical :size="8" style="width: 100%;">
                        <n-space align="center" justify="space-between">
                          <n-tag :type="p115Info?.has_token ? 'success' : 'default'" size="small">
                            <template #icon>
                              <n-icon :component="p115Info?.has_token ? CheckIcon : CloseIcon" />
                            </template>
                            {{ p115Info?.has_token ? `${openApiAuthLabel}` : `未授权：${openApiAuthLabel}` }}
                          </n-tag>
                          <n-button size="small" type="primary" @click="handleOpenQrcodeModal"><template #icon><UiIcon name="key" /></template>
                            {{ p115Info?.has_token ? '重新扫码' : '扫码授权' }}
                          </n-button>
                        </n-space>
                        <n-input-group>
                          <n-input v-model:value="configModel.p115_app_id" placeholder="先保存此卡片的自定义 AppID 再扫码" />
                        </n-input-group>
                        <template #feedback>
                          <n-text depth="3" style="font-size:0.8em;">
                            请先填写 AppID 并保存此卡片，然后再扫码授权。
                          </n-text>
                        </template>
                      </n-space>
                    </n-form-item>

                    <!--  Cookie  -->
                    <n-form-item label="Cookie">
                      <n-space vertical :size="8" style="width: 100%;">
                        <n-space align="center" justify="space-between">
                          <n-tag
                            :type="!p115Info?.has_cookie ? 'default' : (p115Info?.cookie_valid === false ? 'error' : 'success')"
                            size="small"
                          >
                            <template #icon>
                              <n-icon :component="p115Info?.has_cookie ? CheckIcon : CloseIcon" />
                            </template>
                            {{ p115Info?.has_cookie ? cookieAppLabel : '未配置' }}
                          </n-tag>

                          <n-space :size="8">
                            <n-button size="small" type="primary" @click="openCookieModal"><template #icon><UiIcon name="refresh" /></template>
                              {{ p115Info?.has_cookie ? '重新获取' : '扫码获取' }}
                            </n-button>
                            <n-popconfirm
                              v-if="p115Info?.has_cookie"
                              positive-text="清理"
                              negative-text="取消"
                              @positive-click="handleClear115Cookie"
                            >
                              <template #trigger>
                                <n-button size="small" type="error" secondary :loading="clearing115Cookie">
                                  <template #icon><n-icon :component="ClearIcon" /></template>
                                  清理 Cookie
                                </n-button>
                              </template>
                              清理后将保留 OpenAPI Token，但 Cookie 直链缓存会同时失效。确定继续吗？
                            </n-popconfirm>
                          </n-space>
                        </n-space>
                      </n-space>
                    </n-form-item>

                    <n-form-item label="115接口" path="p115_api_priority">
                      <n-radio-group v-model:value="configModel.p115_api_priority" name="api_priority_group">
                        <n-space>
                          <n-radio value="cookie">优先 Cookie（默认）</n-radio>
                          <n-radio value="openapi">优先 OpenAPI（备用）</n-radio>
                        </n-space>
                      </n-radio-group>
                      <template #feedback>
                        <n-text depth="3" style="font-size:0.8em;">默认优先 Cookie，失败后再尝试 OpenAPI。</n-text>
                      </template>
                    </n-form-item>

                    <n-form-item label="API 请求间隔 (秒)" path="p115_request_interval">
                      <n-input-number v-model:value="configModel.p115_request_interval" :min="0.1" :step="0.1" placeholder="0.5" />
                      <template #feedback>
                        <n-text depth="3" style="font-size:0.8em;">
                          115 官方对 API 调用频率有严格限制，建议保持 0.5 秒以上。
                        </n-text>
                      </template>
                    </n-form-item>
                    <template #footer>
                      <n-space justify="end">
                        <n-button type="primary" :loading="savingCard === 'p115Account'" :disabled="savingConfig" aria-label="保存账户与连接" @click="saveCard('p115Account')">
                          <template #icon><n-icon :component="SaveIcon" /></template>
                          保存
                        </n-button>
                      </n-space>
                    </template>
                  </n-card>
                </n-gi>

                <n-gi>
                  <n-card :bordered="false" class="dashboard-card" style="height: 100%;">
                    <template #header><span class="card-title">指纹与缓存</span></template>

                    <n-form-item label="本地 STRM 根目录" path="local_strm_root">
                      <n-input-group>
                        <n-input
                          v-model:value="configModel.local_strm_root"
                          placeholder="例如: /mnt/media"
                          style="flex: 1;"
                        />
                        <n-button type="primary" ghost aria-label="选择本地 STRM 根目录" @click="openLocalFolderSelector('local_strm_root')">
                          <template #icon><n-icon :component="FolderIcon" /></template>
                        </n-button>
                      </n-input-group>
                      <template #feedback>
                        <n-text depth="3" style="font-size:0.8em;">用于从 Emby 路径反查 115 缓存位置，辅助补齐 PC/SHA1。</n-text>
                      </template>
                    </n-form-item>

                    <n-form-item label="CD2 媒体库本地根目录" path="cd2_media_root">
                      <n-input-group>
                        <n-input
                          v-model:value="configModel.cd2_media_root"
                          placeholder="例如: /cd2/115open/媒体库"
                          style="flex: 1;"
                        />
                        <n-button type="primary" ghost aria-label="选择 CD2 媒体库根目录" @click="openLocalFolderSelector('cd2_media_root')">
                          <template #icon><n-icon :component="FolderIcon" /></template>
                        </n-button>
                      </n-input-group>
                    </n-form-item>

                    <n-form-item label="115 云端媒体库根目录" path="p115_fingerprint_media_root_path">
                      <n-input-group>
                        <n-input
                          v-model:value="configModel.p115_fingerprint_media_root_path"
                          placeholder="例如: 媒体库"
                          readonly
                          style="flex: 1;"
                        />
                        <n-button type="primary" ghost aria-label="选择 115 云端目录" @click="openP115FolderSelector">
                          <template #icon><n-icon :component="FolderIcon" /></template>
                        </n-button>
                      </n-input-group>
                    </n-form-item>

                    <n-form-item label="媒体信息格式化" path="p115_generate_mediainfo">
                      <n-switch v-model:value="configModel.p115_generate_mediainfo">
                          <template #checked>写入缓存</template>
                          <template #unchecked>不写入</template>
                      </n-switch>
                      <template #feedback>
                          <n-text depth="3" style="font-size:0.8em;">
                              Webhook 或指纹任务可将媒体信息写入本地缓存，用于备份与还原。
                          </n-text>
                      </template>
                    </n-form-item>
                    <n-form-item label="Emby 删除联动 115 删除" path="p115_enable_sync_delete">
                      <n-switch v-model:value="configModel.p115_enable_sync_delete">
                          <template #checked>已启用</template>
                          <template #unchecked>已关闭</template>
                      </n-switch>
                      <template #feedback>
                          <n-text depth="3" style="font-size:0.8em;">
                              通知到达时同步删除匹配的 115 文件。此操作不可逆，请确认删除链路可靠后再开启。
                          </n-text>
                      </template>
                    </n-form-item>
                    <template #footer>
                      <n-space justify="end">
                        <n-button type="primary" :loading="savingCard === 'fingerprints'" :disabled="savingConfig" aria-label="保存指纹与缓存" @click="saveCard('fingerprints')">
                          <template #icon><n-icon :component="SaveIcon" /></template>
                          保存
                        </n-button>
                      </n-space>
                    </template>
                  </n-card>
                </n-gi>
              </n-grid>
            </n-tab-pane>

            <!-- ================== 标签页 2: Emby (紧凑版) ================== -->
            <n-tab-pane name="emby"><template #tab><span class="mp-title-with-icon"><UiIcon name="media" :size="16" />Emby</span></template>
              <n-grid cols="1" :x-gap="24" :y-gap="24" responsive="screen">

                <!-- ########## 左侧卡片: Emby 连接设置 ########## -->
                <n-gi>
                  <n-card :bordered="false" class="dashboard-card">
                    <template #header><span class="card-title">Emby 连接设置</span></template>

                    <!-- ★★★ 调整点1: 恢复双列，但减小间距 x-gap="12" ★★★ -->
                    <n-grid cols="1 m:2" :x-gap="12" :y-gap="12" responsive="screen">

                      <!-- 1. Emby URL (左) -->
                      <!-- ★★★ 调整点2: label-width="100" 覆盖全局的200，让输入框更长、更紧凑 ★★★ -->
                      <n-form-item-grid-item label="Emby URL" path="emby_server_url" label-width="100">
                        <n-input v-model:value="configModel.emby_server_url" placeholder="http://localhost:8096" />
                      </n-form-item-grid-item>

                      <!-- 2. 外网访问 URL (右) -->
                      <n-form-item-grid-item label="外网URL" path="emby_public_url" label-width="100">
                        <n-input v-model:value="configModel.emby_public_url" placeholder="留空则不开启" />
                      </n-form-item-grid-item>

                      <!-- 3. API Key (左) -->
                      <n-form-item-grid-item label="APIKey" path="emby_api_key" label-width="100">
                        <n-input v-model:value="configModel.emby_api_key" type="password" show-password-on="click" placeholder="输入 API Key" />
                      </n-form-item-grid-item>

                      <n-form-item-grid-item label="Webhook密钥" path="emby_webhook_secret" label-width="100">
                        <n-input v-model:value="configModel.emby_webhook_secret" type="password" show-password-on="click" placeholder="用于 /webhook/emby?token=..." />
                      </n-form-item-grid-item>

                      <n-form-item-grid-item label="Webhook入库" path="emby_webhook_ingest_enabled" span="1 m:2" label-width="100">
                        <n-switch v-model:value="configModel.emby_webhook_ingest_enabled">
                          <template #checked>已开启</template>
                          <template #unchecked>已关闭</template>
                        </n-switch>
                        <template #feedback>
                          <n-text depth="3" style="font-size:0.8em;">
                            关闭后忽略 Emby 的 item.add 和 library.new 入库事件；MoviePilot完成通知、元数据更新和删除通知不受影响。
                          </n-text>
                        </template>
                      </n-form-item-grid-item>

                      <!-- 4. 用户 ID (右) -->
                      <n-form-item-grid-item label="用户ID" :rule="embyUserIdRule" path="emby_user_id" label-width="100">
                        <n-input v-model:value="configModel.emby_user_id" placeholder="32位用户ID" />
                        <template #feedback>
                          <div v-if="isInvalidUserId" style="color: var(--ui-error-color); font-size: 12px;">格式错误！ID应为32位。</div>
                        </template>
                      </n-form-item-grid-item>

                      <!-- 分割线 (占满一行) -->
                      <n-gi span="1 m:2">
                        <n-divider title-placement="left" style="margin: 8px 0; font-size: 0.9em; color: var(--ui-text-muted);">管理员凭证 (选填)</n-divider>
                      </n-gi>

                      <!-- 5. 管理员用户 (左) -->
                      <n-form-item-grid-item label="用户名" path="emby_admin_user" label-width="100">
                        <n-input v-model:value="configModel.emby_admin_user" placeholder="管理员用户名" />
                      </n-form-item-grid-item>

                      <!-- 6. 管理员密码 (右) -->
                      <n-form-item-grid-item label="密码" path="emby_admin_pass" label-width="100">
                        <n-input v-model:value="configModel.emby_admin_pass" type="password" show-password-on="click" placeholder="管理员密码" />
                      </n-form-item-grid-item>

                      <!-- 7. 超时时间 (占满一行，保持长标签) -->
                      <n-form-item-grid-item label="Emby API 超时时间 (秒)" path="emby_api_timeout" span="1 m:2" label-width="200">
                        <n-input-number v-model:value="configModel.emby_api_timeout" :min="15" :step="5" placeholder="建议 30-90" style="width: 100%;" />
                      </n-form-item-grid-item>

                      <!-- 分割线 -->
                      <n-gi span="1 m:2">
                        <n-divider title-placement="left" style="margin-top: 10px;">选择要忽略的媒体库</n-divider>
                      </n-gi>

                      <!-- 8. 媒体库选择 -->
                      <n-form-item-grid-item label-placement="top" span="1 m:2">
                        <n-spin :show="loadingLibraries">
                          <n-space align="center" style="margin-bottom: 12px;">
                            <n-button size="small" :disabled="!availableLibraries.length" @click="configModel.libraries_to_ignore = availableLibraries.map(lib => lib.Id)">
                              <template #icon><n-icon :component="SelectAllIcon" /></template>全选
                            </n-button>
                            <n-button size="small" :disabled="!configModel.libraries_to_ignore?.length" @click="configModel.libraries_to_ignore = []">
                              <template #icon><n-icon :component="ClearIcon" /></template>清空
                            </n-button>
                            <n-text depth="3">默认不选，处理全部媒体库；勾选后忽略对应媒体库。</n-text>
                          </n-space>
                          <n-checkbox-group v-model:value="configModel.libraries_to_ignore">
                            <n-space item-style="display: flex; flex-wrap: wrap;">
                              <n-checkbox v-for="lib in availableLibraries" :key="lib.Id" :value="lib.Id" :label="lib.Name" />
                            </n-space>
                          </n-checkbox-group>
                          <n-text depth="3" v-if="!loadingLibraries && availableLibraries.length === 0 && (configModel.emby_server_url && configModel.emby_api_key)">
                            未找到媒体库。请检查 Emby URL 和 API Key。
                          </n-text>
                          <div v-if="libraryError" style="color: red; margin-top: 5px;">{{ libraryError }}</div>
                        </n-spin>
                      </n-form-item-grid-item>

                    </n-grid>
                    <template #footer>
                      <n-space justify="end">
                        <n-button type="primary" :loading="savingCard === 'emby'" :disabled="savingConfig" aria-label="保存Emby 连接设置" @click="saveCard('emby')">
                          <template #icon><n-icon :component="SaveIcon" /></template>
                          保存
                        </n-button>
                      </n-space>
                    </template>
                  </n-card>
                </n-gi>

              </n-grid>
            </n-tab-pane>

            <!-- ================== 标签页 3: 智能服务  ================== -->
            <n-tab-pane name="services"><template #tab><span class="mp-title-with-icon"><UiIcon name="language" :size="16" />智能服务</span></template>
              <n-grid cols="1" :x-gap="24" :y-gap="24" responsive="screen">

                <!-- 左侧: AI翻译 -->
                <n-gi>
                  <n-card :bordered="false" class="dashboard-card" style="height: 100%;">
                    <template #header><span class="card-title">AI 增强</span></template>
                    <template #header-extra>
                      <n-space align="center">
                        <n-button
                          size="tiny"
                          type="info"
                          ghost
                          @click="openPromptModal"
                        ><template #icon><UiIcon name="language" /></template>
                          配置提示词
                        </n-button>
                        <n-button
                          size="tiny"
                          type="primary"
                          ghost
                          @click="testAI"
                          :loading="isTestingAI"
                          :disabled="!configModel.ai_api_key"
                        ><template #icon><UiIcon name="link" /></template>
                          测试连接
                        </n-button>
                      </n-space>
                    </template>

                    <!-- 移除 content-disabled 类，因为不再有总开关控制禁用 -->
                    <div class="ai-settings-wrapper">

                      <!-- 1. 基础配置 (上移，因为它们是前提) -->
                      <n-form-item label="AI 服务商" path="ai_provider">
                        <n-select v-model:value="configModel.ai_provider" :options="aiProviderOptions" />
                      </n-form-item>
                      <n-form-item label="API Key" path="ai_api_key">
                        <n-input type="password" show-password-on="mousedown" v-model:value="configModel.ai_api_key" placeholder="输入你的 API Key" />
                      </n-form-item>
                      <n-form-item label="模型名称" path="ai_model_name">
                        <n-input-group>
                          <n-select
                            v-model:value="configModel.ai_model_name"
                            :options="aiModelSelectOptions"
                            filterable
                            tag
                            clearable
                            placeholder="点击右侧刷新后选择，或手动输入模型名"
                            style="flex: 1;"
                          />
                          <n-button
                            type="primary"
                            ghost
                            @click="refreshAIModels"
                            :loading="isFetchingAIModels"
                            :disabled="!configModel.ai_api_key"
                          >
                            <template #icon><n-icon :component="RefreshIcon" /></template>
                            刷新
                          </n-button>
                        </n-input-group>
                        <template #feedback>
                          <n-text depth="3" style="font-size:0.8em;">
                            从当前 AI 服务商的 models 接口读取，刷新后可直接下拉选择；仍支持手动输入。
                          </n-text>
                        </template>
                      </n-form-item>
                      <n-form-item label="API Base URL (可选)" path="ai_base_url">
                        <n-input v-model:value="configModel.ai_base_url" placeholder="用于代理或第三方兼容服务" />
                      </n-form-item>

                      <n-grid :cols="isMobile ? 1 : 2" :x-gap="16">
                        <n-form-item-grid-item label="AI 翻译超时时间（秒）" path="ai_request_timeout">
                          <n-input-number
                            v-model:value="configModel.ai_request_timeout"
                            :min="30"
                            :max="1800"
                            :step="30"
                            style="width: 100%;"
                          />
                        </n-form-item-grid-item>
                        <n-form-item-grid-item v-if="configModel.ai_provider === 'openai'" label="失败重试次数" path="ai_max_retries">
                          <n-input-number
                            v-model:value="configModel.ai_max_retries"
                            :min="0"
                            :precision="0"
                            :step="1"
                            style="width: 100%;"
                          />
                          <template #feedback>
                            0 表示不重试。超时或临时请求失败后重试当前请求；已成功翻译的词条会使用缓存。
                          </template>
                        </n-form-item-grid-item>
                        <n-form-item-grid-item label="单批最大翻译词条数" path="ai_translation_batch_size">
                          <n-input-number
                            v-model:value="configModel.ai_translation_batch_size"
                            :min="1"
                            :max="100"
                            :step="1"
                            style="width: 100%;"
                          />
                        </n-form-item-grid-item>
                      </n-grid>

                      <n-divider style="margin: 10px 0; font-size: 0.9em; color: var(--ui-text-muted);">功能开关</n-divider>

                      <!-- 2. 功能细分开关 -->
                      <n-form-item label="启用功能">
                        <n-grid :cols="isMobile ? 1 : 2" :y-gap="8">
                          <n-gi>
                            <n-checkbox v-model:checked="configModel.ai_translate_actor_role">
                              翻译演员与角色
                            </n-checkbox>
                          </n-gi>
                          <n-gi>
                            <n-checkbox v-model:checked="configModel.ai_translate_on_sync">
                              同步媒体数据时翻译
                            </n-checkbox>
                          </n-gi>
                          <n-gi>
                            <n-checkbox v-model:checked="configModel.ai_translate_title">
                              翻译片名
                            </n-checkbox>
                          </n-gi>
                          <n-gi>
                            <n-checkbox v-model:checked="configModel.ai_translate_overview">
                              翻译简介
                            </n-checkbox>
                          </n-gi>
                          <n-gi>
                            <n-checkbox v-model:checked="configModel.ai_translate_episode_overview">
                              翻译分集简介
                            </n-checkbox>
                          </n-gi>
                          <n-gi>
                            <n-checkbox v-model:checked="configModel.ai_joke_fallback">
                              无简介小笑话占位
                            </n-checkbox>
                          </n-gi>
                        </n-grid>
                      </n-form-item>

                      <!-- 3. 高级选项 -->
                      <n-form-item label="翻译模式" path="ai_translation_mode" v-if="configModel.ai_translate_actor_role || configModel.ai_translate_title_overview">
                        <n-radio-group v-model:value="configModel.ai_translation_mode" name="ai_translation_mode">
                          <n-space vertical>
                            <n-radio value="fast">快速模式 (仅翻译)</n-radio>
                            <n-radio value="quality">顾问模式 (结合剧情上下文)</n-radio>
                          </n-space>
                        </n-radio-group>
                      </n-form-item>

                    </div>
                    <template #footer>
                      <n-space justify="end">
                        <n-button type="primary" :loading="savingCard === 'ai'" :disabled="savingConfig" aria-label="保存AI 增强" @click="saveCard('ai')">
                          <template #icon><n-icon :component="SaveIcon" /></template>
                          保存
                        </n-button>
                      </n-space>
                    </template>
                  </n-card>
                </n-gi>

              </n-grid>
            </n-tab-pane>

            <!-- ================== 标签页 4: 高级 (核心修改区域) ================== -->
            <n-tab-pane name="advanced"><template #tab><span class="mp-title-with-icon"><UiIcon name="options" :size="16" />高级</span></template>
              <n-grid class="mp-aligned-grid settings-card-grid" cols="1" :x-gap="24" :y-gap="24">

                <!-- 卡片 1: 日志配置 -->
                <n-gi>
                  <n-card :bordered="false" class="dashboard-card">
                    <template #header><span class="card-title">日志配置</span></template>
                    <n-form-item-grid-item label="单个日志文件大小 (MB)" path="log_rotation_size_mb">
                      <n-input-number v-model:value="configModel.log_rotation_size_mb" :min="1" :step="1" placeholder="例如: 5"/>
                      <template #feedback><n-text depth="3" style="font-size:0.8em;">设置 app.log 文件的最大体积，超限后会轮转。</n-text></template>
                    </n-form-item-grid-item>
                    <n-form-item-grid-item label="日志备份数量" path="log_rotation_backup_count">
                      <n-input-number v-model:value="configModel.log_rotation_backup_count" :min="1" :step="1" placeholder="例如: 10"/>
                      <template #feedback><n-text depth="3" style="font-size:0.8em;">保留最近的日志文件数量 (app.log.1, app.log.2 ...)。</n-text></template>
                    </n-form-item-grid-item>
                    <template #footer>
                      <n-space justify="end">
                        <n-button type="primary" :loading="savingCard === 'logs'" :disabled="savingConfig" aria-label="保存日志配置" @click="saveCard('logs')">
                          <template #icon><n-icon :component="SaveIcon" /></template>
                          保存
                        </n-button>
                      </n-space>
                    </template>
                  </n-card>
                </n-gi>

                <!-- 卡片 2: 入库自动处理 -->
                <n-gi>
                  <n-card :bordered="false" class="dashboard-card">
                    <template #header><span class="card-title">入库自动处理</span></template>
                    <n-form-item-grid-item label="入库自动纠正高清音轨" path="auto_repair_truehd_core_on_ingest">
                      <n-switch v-model:value="configModel.auto_repair_truehd_core_on_ingest" />
                      <template #feedback>
                        <n-text depth="3" style="font-size:0.8em;">
                          仅在入库时执行安全的兼容核心纠正；手动音轨纠正任务不受此开关影响。默认关闭。
                        </n-text>
                      </template>
                    </n-form-item-grid-item>
                    <n-form-item-grid-item label="入库自动填充语言标题" path="auto_fill_language_titles_on_ingest">
                      <n-switch v-model:value="configModel.auto_fill_language_titles_on_ingest" />
                      <template #feedback>
                        <n-text depth="3" style="font-size:0.8em;">
                          为可识别语言的音轨和非中文字幕填充语言标题；中文字幕保留原标题并发出提醒，未知语言需手动确认。默认关闭。
                        </n-text>
                      </template>
                    </n-form-item-grid-item>
                    <template #footer>
                      <n-space justify="end">
                        <n-button type="primary" :loading="savingCard === 'ingest'" :disabled="savingConfig" aria-label="保存入库自动处理" @click="saveCard('ingest')">
                          <template #icon><n-icon :component="SaveIcon" /></template>
                          保存
                        </n-button>
                      </n-space>
                    </template>
                  </n-card>
                </n-gi>

                <!-- 卡片 3: 媒体重命名格式 -->
                <n-gi>
                  <n-card :bordered="false" class="dashboard-card">
                    <template #header><span class="card-title">媒体重命名格式</span></template>
                    <div class="rename-format-grid">
                      <div class="rename-format-form">
                        <n-form-item-grid-item label="电影格式" path="movie_rename_format">
                          <n-input
                            v-model:value="configModel.movie_rename_format"
                            type="textarea"
                            :autosize="{ minRows: 3, maxRows: 8 }"
                            spellcheck="false"
                          />
                          <template #feedback>
                            <n-text depth="3" style="font-size:0.8em;">
                              与 MoviePilot「存储&目录 → 整理&刮削」使用相同的 Jinja2 格式；扩展名始终使用源文件扩展名。
                            </n-text>
                          </template>
                        </n-form-item-grid-item>
                      </div>
                      <div class="rename-preview-panel">
                        <div class="rename-preview-title">电影示例 · 肖申克的救赎</div>
                        <n-spin :show="isPreviewingRenameFormats" size="small">
                          <div v-if="movieRenamePreviewParts.length" class="rename-preview-tree">
                            <div
                              v-for="(part, index) in movieRenamePreviewParts"
                              :key="`${part}-${index}`"
                              class="rename-preview-node"
                              :style="{ paddingLeft: `${index * 22}px` }"
                            >
                              <n-icon :component="index === movieRenamePreviewParts.length - 1 ? FileIcon : FolderIcon" :color="index === movieRenamePreviewParts.length - 1 ? 'var(--ui-info-color)' : 'var(--ui-warning-color)'" />
                              <span>{{ part }}</span>
                            </div>
                          </div>
                          <n-alert v-else :type="renameFormatPreview.error ? 'error' : 'default'" :show-icon="false">
                            {{ renameFormatPreview.error || '正在生成预览...' }}
                          </n-alert>
                        </n-spin>
                      </div>
                      <div class="rename-format-form">
                        <n-form-item-grid-item label="电视剧格式" path="tv_rename_format">
                          <n-input
                            v-model:value="configModel.tv_rename_format"
                            type="textarea"
                            :autosize="{ minRows: 3, maxRows: 8 }"
                            spellcheck="false"
                          />
                          <template #feedback>
                            <n-text depth="3" style="font-size:0.8em;">
                              与 MoviePilot「存储&目录 → 整理&刮削」使用相同的 Jinja2 格式；可使用 `season`、`episode` 和 `season_episode`。
                            </n-text>
                          </template>
                        </n-form-item-grid-item>
                      </div>
                      <div class="rename-preview-panel">
                        <div class="rename-preview-title">电视剧示例 · 凡人修仙传（动漫）</div>
                        <n-spin :show="isPreviewingRenameFormats" size="small">
                          <div v-if="tvRenamePreviewParts.length" class="rename-preview-tree">
                            <div
                              v-for="(part, index) in tvRenamePreviewParts"
                              :key="`${part}-${index}`"
                              class="rename-preview-node"
                              :style="{ paddingLeft: `${index * 22}px` }"
                            >
                              <n-icon :component="index === tvRenamePreviewParts.length - 1 ? FileIcon : FolderIcon" :color="index === tvRenamePreviewParts.length - 1 ? 'var(--ui-info-color)' : 'var(--ui-warning-color)'" />
                              <span>{{ part }}</span>
                            </div>
                          </div>
                          <n-alert v-else :type="renameFormatPreview.error ? 'error' : 'default'" :show-icon="false">
                            {{ renameFormatPreview.error || '正在生成预览...' }}
                          </n-alert>
                        </n-spin>
                      </div>
                    </div>
                    <template #footer>
                      <n-space justify="end">
                        <n-button type="primary" :loading="savingCard === 'rename'" :disabled="savingConfig" aria-label="保存媒体重命名格式" @click="saveCard('rename')">
                          <template #icon><n-icon :component="SaveIcon" /></template>
                          保存
                        </n-button>
                      </n-space>
                    </template>
                  </n-card>
                </n-gi>

                <!-- 卡片 3: 数据管理 (左下) -->
                <n-gi>
                  <n-card :bordered="false" class="dashboard-card">
                    <template #header><span class="card-title">数据管理</span></template>
                    <n-space vertical>
                      <div class="data-action-grid">
                        <n-button @click="showExportModal" :loading="isExporting" class="action-button"><template #icon><n-icon :component="ExportIcon" /></template>导出数据</n-button>
                        <n-upload :custom-request="handleCustomImportRequest" :show-file-list="false" accept=".json.gz"><n-button :loading="isImporting" class="action-button"><template #icon><n-icon :component="ImportIcon" /></template>导入数据</n-button></n-upload>
                        <n-button @click="showClearTablesModal" :loading="isClearing" class="action-button" type="error" ghost><template #icon><n-icon :component="ClearIcon" /></template>清空指定表</n-button>
                        <n-popconfirm @positive-click="handleCleanupOfflineMedia">
                          <template #trigger>
                            <n-button type="warning" ghost :loading="isCleaningOfflineMedia" class="action-button">
                              <template #icon><n-icon :component="ClearIcon" /></template>
                              清理离线媒体
                            </n-button>
                          </template>
                          <div style="max-width: 300px">
                            <p style="margin: 0 0 4px 0;">确定要清理离线媒体数据吗？</p>
                            <p style="margin: 0 0 4px 0;">这将删除所有 <b>不在库</b> 的元数据缓存。</p>
                            <span style="font-size: 0.9em; color: var(--ui-text-muted);">此操作用于数据库瘦身，不会影响已入库媒体项。</span>
                          </div>
                        </n-popconfirm>
                        <n-popconfirm @positive-click="handleCorrectSequences">
                          <template #trigger>
                            <n-button type="warning" ghost :loading="isCorrecting" class="action-button">
                              <template #icon><n-icon :component="BuildIcon" /></template>
                              校准ID计数器
                            </n-button>
                          </template>
                          确定要校准所有表的ID自增计数器吗？<br />
                          这是一个安全的操作，用于修复导入数据后无法新增条目的问题。
                        </n-popconfirm>
                        <!-- ### 重置演员映射表 ### -->
                        <n-button
                          type="warning"
                          ghost
                          :loading="isResettingMappings"
                          class="action-button"
                          @click="showResetMappingsModal"
                        >
                          <template #icon><n-icon :component="SyncIcon" /></template>
                          重置Emby数据
                        </n-button>
                      </div>
                      <p class="description-text"><b>导出：</b>将数据库中的一个或多个表备份为 JSON.GZ 文件。<br><b>导入：</b>从 JSON.GZ 备份文件中恢复数据。<br><b>清空：</b>删除指定表中的所有数据，此操作不可逆。<br><b>清理离线：</b>移除已不在库的残留元数据记录，给数据库瘦身。<br><b>校准：</b>修复导入数据可能引起的自增序号错乱的问题。<br><b>重置：</b>在重建 Emby 媒体库后，使用此功能清空所有旧的 Emby 关联数据（用户、播放状态等），并保留核心元数据，以便后续重新扫描和关联。</p>
                    </n-space>
                  </n-card>
                </n-gi>


              </n-grid>
            </n-tab-pane>
          </n-tabs>
        </n-form>
      </div>

      <n-alert v-else-if="configError" title="加载配置失败" type="error">
        {{ configError }}
      </n-alert>

      <div v-else>
        正在加载配置...
      </div>

    </n-space>

    <!-- ★★★ Cookie 扫码获取弹窗 ★★★ -->
    <n-modal v-model:show="showCookieModal" preset="card" title="扫码获取 Cookie" :style="modalStyle(400)" :mask-closable="false" class="custom-modal glass-modal">
      <n-space vertical>
        <n-alert type="info" :show-icon="true" style="margin-bottom: 10px;">
          本次二维码会绑定所选客户端，获取 Cookie 后还会校验实际客户端是否一致。
        </n-alert>

        <n-space align="center">
          <n-select
            v-model:value="cookieAppType"
            :options="cookieAppOptions"
            style="width: 220px;"
            @update:value="refreshCookieQrcode"
          />
          <n-button type="primary" @click="refreshCookieQrcode" :loading="cookieQrcodeLoading"><template #icon><UiIcon name="refresh" /></template>
            刷新二维码
          </n-button>
        </n-space>

        <div style="text-align: center; padding: 20px 0; min-height: 250px;">
          <n-spin v-if="cookieQrcodeStatus === 'loading'" size="large">
            <template #description>正在获取二维码...</template>
          </n-spin>

          <div v-else-if="cookieQrcodeStatus === 'waiting' || cookieQrcodeStatus === 'success'">
            <n-qr-code
              v-if="cookieQrcodeUrl"
              :value="cookieQrcodeUrl"
              :size="200"
              style="margin: 0 auto 20px;"
            />
            <n-alert v-if="cookieQrcodeStatus === 'waiting'" type="info" :show-icon="true">
              请使用 {{ selectedCookieAppLabel }} 扫描二维码并确认登录
            </n-alert>
            <n-alert v-if="cookieQrcodeStatus === 'success'" type="success" :show-icon="true">
              获取成功！Cookie 已自动保存到数据库
            </n-alert>
          </div>

          <div v-else-if="cookieQrcodeStatus === 'expired'">
            <n-result status="warning" title="二维码已过期">
              <template #footer>
                <n-button type="primary" @click="refreshCookieQrcode"><template #icon><UiIcon name="refresh" /></template>重新获取</n-button>
              </template>
            </n-result>
          </div>

          <div v-else-if="cookieQrcodeStatus === 'error'">
            <n-result status="error" title="Cookie 获取失败">
              <template #footer>
                <n-button type="primary" @click="refreshCookieQrcode"><template #icon><UiIcon name="refresh" /></template>重新获取</n-button>
              </template>
            </n-result>
          </div>
        </div>

        <n-divider style="margin: 0;" />
        <n-collapse>
          <n-collapse-item title="手动粘贴 Cookie (备用方案)" name="manual">
            <n-input
              v-model:value="tempCookieInput"
              type="textarea"
              placeholder="UID=...; CID=...; SEID=..."
              :rows="3"
            />
            <n-button type="primary" size="small" style="margin-top: 8px;" @click="saveManualCookie"><template #icon><UiIcon name="save" /></template>
              保存手动输入的 Cookie
            </n-button>
          </n-collapse-item>
        </n-collapse>
      </n-space>

      <template #footer>
        <n-button @click="closeCookieModal" v-if="cookieQrcodeStatus !== 'success'">关闭</n-button>
      </template>
    </n-modal>

    <!-- ★★★ 115 扫码登录弹窗 ★★★ -->
    <n-modal v-model:show="showQrcodeModal" preset="card" title="115 扫码登录" :style="modalStyle(350)" :mask-closable="false" class="custom-modal glass-modal">
      <div style="text-align: center; padding: 20px 0;">
        <!-- 加载中状态 -->
        <n-spin v-if="qrcodeStatus === 'loading'" size="large">
          <template #description>正在获取二维码...</template>
        </n-spin>

        <!-- 二维码显示 -->
        <div v-else-if="qrcodeStatus === 'waiting' || qrcodeStatus === 'success'">
          <n-qr-code
            v-if="qrcodeUrl"
            :value="qrcodeUrl"
            :size="200"
            style="margin: 0 auto 20px;"
          />
          <n-alert v-if="qrcodeStatus === 'waiting'" type="info" :show-icon="true">
            请使用 115 APP 扫描二维码登录
          </n-alert>
          <n-alert v-if="qrcodeStatus === 'success'" type="success" :show-icon="true">
            登录成功！ Cookies 已自动保存
          </n-alert>
        </div>

        <!-- 过期状态 -->
        <div v-else-if="qrcodeStatus === 'expired'">
          <n-result status="warning" title="二维码已过期">
            <template #footer>
              <n-button type="primary" @click="openQrcodeModal"><template #icon><UiIcon name="refresh" /></template>重新获取二维码</n-button>
            </template>
          </n-result>
        </div>

        <!-- 错误状态 -->
        <div v-else-if="qrcodeStatus === 'error'">
          <n-result status="error" title="获取二维码失败">
            <template #footer>
              <n-button type="primary" @click="openQrcodeModal"><template #icon><UiIcon name="refresh" /></template>重试</n-button>
            </template>
          </n-result>
        </div>
      </div>
      <template #footer>
        <n-button @click="closeQrcodeModal" v-if="qrcodeStatus !== 'success'">关闭</n-button>
      </template>
    </n-modal>
    <!-- ★★★ 本地物理目录选择器弹窗 ★★★ -->
    <n-modal v-model:show="showLocalFolderModal" preset="card" title="选择本地路径" :style="modalStyle(600)" class="custom-modal glass-modal">
      <n-spin :show="loadingLocalFolders">
        <n-space vertical>
          <!-- 顶部路径输入与刷新 -->
          <n-input-group>
            <n-input v-model:value="currentLocalPath" placeholder="当前路径" @keyup.enter="fetchLocalFolders(currentLocalPath)" />
            <n-button type="primary" @click="fetchLocalFolders(currentLocalPath)">
              <template #icon><n-icon :component="RefreshIcon" /></template>
            </n-button>
          </n-input-group>

          <!-- 目录列表 -->
          <n-list hoverable clickable bordered style="max-height: 400px; overflow-y: auto; border-radius: 6px;">
            <n-list-item v-for="folder in localFolders" :key="folder.path" @click="selectLocalFolder(folder)">
              <template #prefix>
                <n-icon :component="folder.is_parent ? ArrowUpIcon : FolderIcon" size="22" :color="folder.is_parent ? 'var(--ui-text-muted)' : 'var(--ui-warning-color)'" />
              </template>
              <span :style="{ fontWeight: folder.is_parent ? 'bold' : 'normal' }">{{ folder.name }}</span>
            </n-list-item>
            <n-empty v-if="localFolders.length === 0" description="空目录或无权限访问" style="margin-top: 30px; margin-bottom: 30px;" />
          </n-list>

          <!-- 底部按钮 -->
          <div style="display: flex; justify-content: flex-end; gap: 12px; margin-top: 16px;">
            <n-button @click="showLocalFolderModal = false">取消</n-button>
            <n-button type="primary" @click="confirmLocalFolder"><template #icon><UiIcon name="select" /></template>确定选择此目录</n-button>
          </div>
        </n-space>
      </n-spin>
    </n-modal>

    <n-modal v-model:show="showP115FolderModal" preset="card" title="选择 115 云端目录" :style="modalStyle(600)" class="custom-modal glass-modal">
      <n-spin :show="loadingP115Folders">
        <n-space vertical>
          <n-input-group>
            <n-input :value="currentP115Path || '根目录'" readonly />
            <n-button type="primary" @click="fetchP115Folders(currentP115Cid)">
              <template #icon><n-icon :component="RefreshIcon" /></template>
            </n-button>
          </n-input-group>

          <n-list hoverable clickable bordered style="max-height: 400px; overflow-y: auto; border-radius: 6px;">
            <n-list-item v-for="folder in p115Folders" :key="`${folder.cid}-${folder.path}`" @click="selectP115Folder(folder)">
              <template #prefix>
                <n-icon :component="folder.is_parent ? ArrowUpIcon : FolderIcon" size="22" :color="folder.is_parent ? 'var(--ui-text-muted)' : 'var(--ui-warning-color)'" />
              </template>
              <span :style="{ fontWeight: folder.is_parent ? 'bold' : 'normal' }">{{ folder.name }}</span>
            </n-list-item>
            <n-empty v-if="p115Folders.length === 0" description="空目录" style="margin-top: 30px; margin-bottom: 30px;" />
          </n-list>

          <div style="display: flex; justify-content: flex-end; gap: 12px; margin-top: 16px;">
            <n-button @click="showP115FolderModal = false">取消</n-button>
            <n-button type="primary" @click="confirmP115Folder"><template #icon><UiIcon name="select" /></template>确定选择此目录</n-button>
          </div>
        </n-space>
      </n-spin>
    </n-modal>
  </n-layout>

  <!-- 导出选项模态框 -->
  <n-modal v-model:show="exportModalVisible" preset="dialog" title="选择要导出的数据表" :style="modalStyle(520)" class="glass-modal">
    <n-space justify="end" style="margin-bottom: 10px;">
      <n-button text type="primary" @click="selectAllForExport"><template #icon><UiIcon name="select" /></template>全选</n-button>
      <n-button text type="primary" @click="deselectAllForExport">全不选</n-button>
    </n-space>
    <n-checkbox-group v-model:value="tablesToExport" vertical>
      <n-grid :y-gap="8" :cols="isMobile ? 1 : 2">
        <n-gi v-for="table in allDbTables" :key="table">
          <n-checkbox :value="table">
            {{ tableInfo[table]?.cn || table }}
          </n-checkbox>
        </n-gi>
      </n-grid>
    </n-checkbox-group>
    <template #action>
      <n-button @click="exportModalVisible = false">取消</n-button>
      <n-button type="primary" @click="handleExport" :disabled="tablesToExport.length === 0"><template #icon><UiIcon name="download" /></template>确认导出</n-button>
    </template>
  </n-modal>
  <!-- 导入选项模态框 -->
  <n-modal v-model:show="importModalVisible" preset="dialog" title="恢复数据库备份" :style="modalStyle(520)" class="glass-modal">
    <n-space vertical>
      <div><p><strong>文件名:</strong> {{ fileToImport?.name }}</p></div>

      <n-alert title="高危操作警告" type="warning">
        此操作将使用备份文件中的数据 <strong class="warning-text">覆盖</strong> 数据库中对应的表。这是一个 <strong class="warning-text">不可逆</strong> 的过程！<br>
        <strong>请确认备份文件来源可靠</strong>，跨环境导入可能导致数据错乱。
      </n-alert>

      <div>
        <n-text strong>选择要恢复的表 (从文件中自动读取)</n-text>
        <n-space style="margin-left: 20px; display: inline-flex; vertical-align: middle;">
          <n-button size="tiny" text type="primary" @click="selectAllForImport"><template #icon><UiIcon name="select" /></template>全选</n-button>
          <n-button size="tiny" text type="primary" @click="deselectAllForImport">全不选</n-button>
        </n-space>
      </div>
      <n-checkbox-group
        v-model:value="tablesToImport"
        @update:value="handleImportSelectionChange"
        vertical
        style="margin-top: 8px;"
      >
        <n-grid :y-gap="8" :cols="isMobile ? 1 : 2">
          <n-gi v-for="table in tablesInBackupFile" :key="table">
            <n-checkbox :value="table">
              {{ tableInfo[table]?.cn || table }}
            </n-checkbox>
          </n-gi>
        </n-grid>
      </n-checkbox-group>
    </n-space>
    <template #action>
      <n-button @click="cancelImport">取消</n-button>
      <n-button type="primary" @click="confirmImport" :disabled="tablesToImport.length === 0"><template #icon><UiIcon name="refresh" /></template>确认并开始恢复</n-button>
    </template>
  </n-modal>

  <!-- 清空指定表模态框 -->
  <n-modal v-model:show="clearTablesModalVisible" preset="dialog" title="清空指定数据表" :style="modalStyle(520)" class="glass-modal">
    <n-space justify="end" style="margin-bottom: 10px;">
      <n-button text type="primary" @click="selectAllForClear"><template #icon><UiIcon name="select" /></template>全选</n-button>
      <n-button text type="primary" @click="deselectAllForClear">全不选</n-button>
    </n-space>
    <n-alert title="高危操作警告" type="error" style="margin-bottom: 15px;">
      此操作将 <strong class="warning-text">永久删除</strong> 所选表中的所有数据，且 <strong class="warning-text">不可恢复</strong>！请务必谨慎操作。
    </n-alert>
    <n-checkbox-group
        v-model:value="tablesToClear"
        @update:value="handleClearSelectionChange"
        vertical
      >
      <n-grid :y-gap="8" :cols="isMobile ? 1 : 2">
        <n-gi v-for="table in allDbTables" :key="table">
          <n-checkbox :value="table">
            {{ tableInfo[table]?.cn || table }}
          </n-checkbox>
        </n-gi>
      </n-grid>
    </n-checkbox-group>
    <template #action>
      <n-button @click="clearTablesModalVisible = false">取消</n-button>
      <n-button type="error" @click="handleClearTables" :disabled="tablesToClear.length === 0" :loading="isClearing"><template #icon><UiIcon name="delete" /></template>确认清空</n-button>
    </template>
  </n-modal>

  <!-- 重置演员映射模态框 -->
  <n-modal
    v-model:show="resetMappingsModalVisible"
    preset="dialog"
    title="确认重置Emby数据"
    :style="modalStyle(520)"
    class="glass-modal"
  >
    <n-alert title="高危操作警告" type="warning" style="margin-bottom: 15px;">
      <p style="margin: 0 0 8px 0;">此操作将 <strong>清空所有Emby相关数据</strong>。</p>
      <p style="margin: 0 0 8px 0;">它会保留宝贵的 元数据以及演员映射，以便在全量扫描后自动重新关联。</p>
      <p class="warning-text" style="margin: 0;"><strong>请仅在您已经或将要重建 Emby 媒体库时执行此操作。</strong></p>
    </n-alert>
    <template #action>
      <n-button @click="resetMappingsModalVisible = false">取消</n-button>
      <n-button type="warning" @click="handleResetActorMappings" :loading="isResettingMappings"><template #icon><UiIcon name="settings" /></template>确认重置</n-button>
    </template>
  </n-modal>
  <!-- AI 提示词配置模态框 -->
  <n-modal v-model:show="promptModalVisible" preset="dialog" title="配置 AI 提示词" :style="modalStyle(800)" class="glass-modal">
    <n-alert type="info" style="margin-bottom: 16px;">
      您可以自定义发送给 AI 的系统指令（System Prompt）。<br>
      <b>注意：</b> 请保留关键的 JSON 输出格式要求，否则会导致解析失败。支持使用 <code>{title}</code> 等占位符。
    </n-alert>

    <n-spin :show="loadingPrompts">
      <n-tabs type="line" size="small" animated>
        <n-tab-pane name="fast_mode"><template #tab><span class="mp-title-with-icon"><UiIcon name="settings" :size="16" />快速模式 (人名)</span></template>
          <n-input
            v-model:value="promptsModel.fast_mode"
            type="textarea"
            :autosize="{ minRows: 10, maxRows: 20 }"
            placeholder="输入提示词..."
            style="font-family: monospace;"
          />
        </n-tab-pane>
        <n-tab-pane name="quality_mode"><template #tab><span class="mp-title-with-icon"><UiIcon name="settings" :size="16" />顾问模式 (人名)</span></template>
          <n-input
            v-model:value="promptsModel.quality_mode"
            type="textarea"
            :autosize="{ minRows: 10, maxRows: 20 }"
            placeholder="输入提示词..."
            style="font-family: monospace;"
          />
        </n-tab-pane>
        <n-tab-pane name="overview_translation"><template #tab><span class="mp-title-with-icon"><UiIcon name="settings" :size="16" />简介翻译</span></template>
          <n-input
            v-model:value="promptsModel.overview_translation"
            type="textarea"
            :autosize="{ minRows: 10, maxRows: 20 }"
            placeholder="输入提示词..."
            style="font-family: monospace;"
          />
          <n-text depth="3" style="font-size: 12px;">可用变量: {title}, {overview}</n-text>
        </n-tab-pane>
        <n-tab-pane name="title_translation"><template #tab><span class="mp-title-with-icon"><UiIcon name="settings" :size="16" />标题翻译</span></template>
          <n-input
            v-model:value="promptsModel.title_translation"
            type="textarea"
            :autosize="{ minRows: 10, maxRows: 20 }"
            placeholder="输入提示词..."
            style="font-family: monospace;"
          />
          <n-text depth="3" style="font-size: 12px;">可用变量: {media_type}, {title}, {year}</n-text>
        </n-tab-pane>
        <n-tab-pane name="transliterate_mode"><template #tab><span class="mp-title-with-icon"><UiIcon name="settings" :size="16" />音译模式</span></template>
          <n-input
            v-model:value="promptsModel.transliterate_mode"
            type="textarea"
            :autosize="{ minRows: 10, maxRows: 20 }"
            placeholder="输入提示词..."
            style="font-family: monospace;"
          />
        </n-tab-pane>
        <n-tab-pane name="batch_joke_fallback"><template #tab><span class="mp-title-with-icon"><UiIcon name="settings" :size="16" />占位简介</span></template>
          <n-input
            v-model:value="promptsModel.batch_joke_fallback"
            type="textarea"
            :autosize="{ minRows: 10, maxRows: 20 }"
            placeholder="输入提示词..."
            style="font-family: monospace;"
          />
        </n-tab-pane>
      </n-tabs>
    </n-spin>

    <template #action>
      <n-space justify="space-between" style="width: 100%">
        <n-popconfirm @positive-click="resetPrompts">
          <template #trigger>
            <n-button type="warning" ghost :loading="savingPrompts"><template #icon><UiIcon name="refresh" /></template>恢复默认</n-button>
          </template>
          确定要丢弃所有自定义修改，恢复到系统默认提示词吗？
        </n-popconfirm>

        <n-space>
          <n-button @click="promptModalVisible = false">取消</n-button>
          <n-button type="primary" @click="savePrompts" :loading="savingPrompts"><template #icon><UiIcon name="save" /></template>保存配置</n-button>
        </n-space>
      </n-space>
    </template>
  </n-modal>
</template>

<script setup>
import { ref, watch, computed, onMounted, onUnmounted, nextTick } from 'vue';
import {
  NCard, NForm, NFormItem, NInputNumber, NSwitch, NButton, NGrid, NGi,
  NSpin, NAlert, NInput, NSelect, NSpace, useMessage, useDialog,
  NFormItemGridItem, NCheckboxGroup, NCheckbox, NText, NRadioGroup, NRadio,
  NTag, NIcon, NUpload, NModal, NDivider, NInputGroup, NTabs, NTabPane, NTooltip,
  NQrCode, NResult, NPageHeader
} from 'naive-ui';
import {
  DownloadOutline as ExportIcon,
  CloudUploadOutline as ImportIcon,
  TrashOutline as ClearIcon,
  BuildOutline as BuildIcon,
  SyncOutline as SyncIcon,
  Folder as FolderIcon,
  CheckmarkCircleOutline as CheckIcon,
  CloseCircleOutline as CloseIcon,
  ArrowUpOutline as ArrowUpIcon,
  RefreshOutline as RefreshIcon,
  DocumentTextOutline as FileIcon,
  SaveOutline as SaveIcon,
  CheckmarkDoneOutline as SelectAllIcon,
} from '@vicons/ionicons5';
import { useConfig } from '../../composables/useConfig.js';
import axios from 'axios';

const MOBILE_BREAKPOINT = 768;
const isMobile = ref(false);

const updateViewportState = () => {
  if (typeof window === 'undefined') return;
  isMobile.value = window.innerWidth <= MOBILE_BREAKPOINT;
};

const pageContentStyle = computed(() => ({
  padding: '0'
}));

const pageSpaceStyle = computed(() => ({
  marginTop: '0'
}));

const pageSpaceSize = computed(() => (isMobile.value ? 12 : 24));

const tabPaneStyle = computed(() => ({
  padding: '20px 0 0',
  boxSizing: 'border-box'
}));

const modalStyle = (width) => ({
  width: isMobile.value ? 'calc(100vw - 24px)' : `${width}px`,
  maxWidth: 'calc(100vw - 24px)'
});
const promptModalVisible = ref(false);
const loadingPrompts = ref(false);
const savingPrompts = ref(false);
const promptsModel = ref({
  fast_mode: '',
  quality_mode: '',
  overview_translation: '',
  title_translation: '',
  transliterate_mode: '',
  batch_joke_fallback: ''
});

const tableInfo = {
  'app_settings': { cn: '基础配置' },
  'person_metadata': { cn: '人物元数据' },
  'translation_cache': { cn: '翻译缓存' },
  'processed_log': { cn: '已处理日志' },
  'failed_log': { cn: '待复核日志' },
  'media_metadata': { cn: '媒体元数据' },
  'emby_users': { cn: 'Emby用户' },
  'emby_users_extended': { cn: 'Emby用户扩展信息' },
  'p115_filesystem_cache': { cn: '115目录缓存' },
  'p115_mediainfo_cache': {cn: '媒体信息备份' },
};
const tableDependencies = {
  'emby_users': ['emby_users_extended'],
};
const reverseTableDependencies = {};
for (const parent in tableDependencies) {
  for (const child of tableDependencies[parent]) {
    reverseTableDependencies[child] = parent;
  }
}
const handleClearSelectionChange = (currentSelection) => {
  const selectionSet = new Set(currentSelection);
  for (const parentTable in tableDependencies) {
    if (selectionSet.has(parentTable)) {
      const children = tableDependencies[parentTable];
      for (const childTable of children) {
        if (!selectionSet.has(childTable)) {
          selectionSet.add(childTable);
        }
      }
    }
  }
  if (selectionSet.size !== tablesToClear.value.length) {
    tablesToClear.value = Array.from(selectionSet);
  }
};
const handleImportSelectionChange = (currentSelection) => {
  const selectionSet = new Set(currentSelection);
  let changed = true;
  while (changed) {
    changed = false;
    const originalSize = selectionSet.size;
    for (const parentTable in tableDependencies) {
      if (selectionSet.has(parentTable)) {
        for (const childTable of tableDependencies[parentTable]) {
          selectionSet.add(childTable);
        }
      }
    }
    for (const childTable in reverseTableDependencies) {
      if (selectionSet.has(childTable)) {
        const parentTable = reverseTableDependencies[childTable];
        selectionSet.add(parentTable);
      }
    }
    if (selectionSet.size > originalSize) {
      changed = true;
    }
  }
  if (selectionSet.size !== tablesToImport.value.length) {
    tablesToImport.value = Array.from(selectionSet);
  }
};

const formRef = ref(null);
const formRules = { trigger: ['input', 'blur'] };
const { configModel, loadingConfig, savingConfig, configError, handleSaveConfig } = useConfig();
const isPreviewingRenameFormats = ref(false);
const renameFormatPreview = ref({ movie: '', tv: '' });
const movieRenamePreviewParts = computed(() => String(renameFormatPreview.value.movie || '').split('/').filter(Boolean));
const tvRenamePreviewParts = computed(() => String(renameFormatPreview.value.tv || '').split('/').filter(Boolean));
let renamePreviewTimer = null;
const subtitleChineseDefaultLanguageOptions = [
  { label: '简体', value: 'chs' },
  { label: '中文', value: 'zho' },
];
const message = useMessage();
const dialog = useDialog();
const isResettingMappings = ref(false);
const resetMappingsModalVisible = ref(false);
const availableLibraries = ref([]);
const loadingLibraries = ref(false);
const libraryError = ref(null);
const componentIsMounted = ref(false);
let unwatchGlobal = null;
let unwatchEmbyConfig = null;
const embyUserIdRegex = /^[a-f0-9]{32}$/i;
const isCleaningOfflineMedia = ref(false);
const isTestingAI = ref(false);
const isInvalidUserId = computed(() => {
  if (!configModel.value || !configModel.value.emby_user_id) return false;
  return configModel.value.emby_user_id.trim() !== '' && !embyUserIdRegex.test(configModel.value.emby_user_id);
});
const embyUserIdRule = {
  configField: 'emby_user_id',
  trigger: ['input', 'blur'],
  validator(rule, value) {
    if (value && !embyUserIdRegex.test(value)) {
      return new Error('ID格式不正确，应为32位。');
    }
    return true;
  }
};
const showResetMappingsModal = () => { resetMappingsModalVisible.value = true; };
const handleResetActorMappings = async () => {
  isResettingMappings.value = true;
  try {
    const response = await axios.post('/api/actions/prepare-for-library-rebuild');
    message.success(response.data.message || 'Emby数据已成功重置！');
    resetMappingsModalVisible.value = false;
  } catch (error) {
    message.error(error.response?.data?.error || '重置失败，请检查后端日志。');
  } finally {
    isResettingMappings.value = false;
  }
};
const refreshAIModels = async () => {
  if (!configModel.value?.ai_api_key) {
    message.warning('请先填写 API Key 再刷新模型列表。');
    return;
  }

  isFetchingAIModels.value = true;
  try {
    const response = await axios.post('/api/ai/models', configModel.value);
    const models = Array.isArray(response.data?.models) ? response.data.models : [];

    aiModelOptions.value = models.map(model => ({
      label: model,
      value: model
    }));

    if (models.length === 0) {
      message.warning('接口连接成功，但没有返回可用模型。');
      return;
    }

    if (!configModel.value.ai_model_name && models.length === 1) {
      configModel.value.ai_model_name = models[0];
    }

    message.success(`已刷新 ${models.length} 个模型，请在下拉框中选择。`);
  } catch (error) {
    const errorMsg = error.response?.data?.message || error.message;
    message.error(`刷新模型失败: ${errorMsg}`);
  } finally {
    isFetchingAIModels.value = false;
  }
};

watch(
  () => [
    configModel.value?.ai_provider,
    configModel.value?.ai_base_url,
    configModel.value?.ai_api_key
  ],
  () => {
    aiModelOptions.value = [];
  }
);

const testAI = async () => {
  if (!configModel.value.ai_api_key) {
    message.warning('请先填写 API Key 再进行测试。');
    return;
  }

  isTestingAI.value = true;
  try {
    // 将当前的 configModel 传给后端进行即时测试
    const response = await axios.post('/api/ai/test', configModel.value);

    if (response.data.success) {
      // 使用 dialog 弹出详细结果，看起来更专业
      dialog.success({
        title: 'AI 测试成功',
        content: response.data.message,
        positiveText: '太棒了'
      });
    } else {
      message.error(`测试失败: ${response.data.message}`);
    }
  } catch (error) {
    const errorMsg = error.response?.data?.message || error.message;
    dialog.error({
      title: 'AI 测试失败',
      content: errorMsg,
      positiveText: '好吧'
    });
  } finally {
    isTestingAI.value = false;
  }
};
const openPromptModal = async () => {
  promptModalVisible.value = true;
  loadingPrompts.value = true;
  try {
    const response = await axios.get('/api/ai/prompts');
    promptsModel.value = response.data;
  } catch (error) {
    message.error('加载提示词失败');
  } finally {
    loadingPrompts.value = false;
  }
};

const savePrompts = async () => {
  savingPrompts.value = true;
  try {
    await axios.post('/api/ai/prompts', promptsModel.value);
    message.success('提示词已保存');
    promptModalVisible.value = false;
  } catch (error) {
    message.error('保存失败');
  } finally {
    savingPrompts.value = false;
  }
};

const resetPrompts = async () => {
  savingPrompts.value = true;
  try {
    const response = await axios.post('/api/ai/prompts/reset');
    promptsModel.value = response.data.prompts;
    message.success('已恢复默认提示词');
  } catch (error) {
    message.error('重置失败');
  } finally {
    savingPrompts.value = false;
  }
};
watch(() => configModel.value?.refresh_emby_after_update, (isRefreshEnabled) => {
  if (configModel.value && !isRefreshEnabled) {
    configModel.value.auto_lock_cast_after_update = false;
  }
});
const aiProviderOptions = ref([
  { label: 'OpenAI (及兼容服务)', value: 'openai' },
  { label: '智谱AI (ZhipuAI)', value: 'zhipuai' },
  { label: 'Google Gemini', value: 'gemini' },
]);

const isFetchingAIModels = ref(false);
const aiModelOptions = ref([]);
const aiModelSelectOptions = computed(() => {
  const currentModel = (configModel.value?.ai_model_name || '').trim();
  if (!currentModel) return aiModelOptions.value;

  const exists = aiModelOptions.value.some(option => option.value === currentModel);
  if (exists) return aiModelOptions.value;

  return [
    { label: currentModel, value: currentModel },
    ...aiModelOptions.value
  ];
});
const isExporting = ref(false);
const exportModalVisible = ref(false);
const allDbTables = ref([]);
const tablesToExport = ref([]);
const isImporting = ref(false);
const importModalVisible = ref(false);
const fileToImport = ref(null);
const tablesInBackupFile = ref([]);
const tablesToImport = ref([]);
const clearTablesModalVisible = ref(false);
const tablesToClear = ref([]);
const isClearing = ref(false);
const isCorrecting = ref(false);
const showClearTablesModal = async () => {
  try {
    const response = await axios.get('/api/database/tables');
    allDbTables.value = response.data;
    tablesToClear.value = [];
    clearTablesModalVisible.value = true;
  } catch (error) {
    message.error('无法获取数据库表列表，请检查后端日志。');
  }
};
const handleClearTables = async () => {
  if (tablesToClear.value.length === 0) {
    message.warning('请至少选择一个要清空的数据表。');
    return;
  }
  isClearing.value = true;
  try {
    const response = await axios.post('/api/actions/clear_tables', { tables: tablesToClear.value });
    message.success(response.data.message || '成功清空所选数据表！');
    clearTablesModalVisible.value = false;
    tablesToClear.value = [];
  } catch (error) {
    const errorMsg = error.response?.data?.error || '清空操作失败，请检查后端日志。';
    message.error(errorMsg);
  } finally {
    isClearing.value = false;
  }
};
const selectAllForClear = () => tablesToClear.value = [...allDbTables.value];
const deselectAllForClear = () => tablesToClear.value = [];

// ★★★ 115 相关逻辑 ★★★
const p115Info = ref(null);
const loading115Info = ref(false);

// ★★★ Cookie 扫码获取逻辑 ★★★
const showCookieModal = ref(false);
const tempCookieInput = ref('');
const cookieAppType = ref('alipaymini');
const cookieAppOptions = [
  { label: '支付宝', value: 'alipaymini' },
  { label: '微信', value: 'wechatmini' },
  { label: '安卓', value: '115android' },
  { label: 'iOS', value: '115ios' },
  { label: '网页', value: 'web' },
  { label: 'PAD', value: '115ipad' },
  { label: 'TV', value: 'tv' }
];

const cookieQrcodeUrl = ref('');
const cookieQrcodeStatus = ref('idle');
const cookieQrcodeLoading = ref(false);
const cookieQrcodePolling = ref(null);
const cookieQrcodeSessionId = ref('');
const clearing115Cookie = ref(false);
let cookieQrcodeRequestVersion = 0;

const selectedCookieAppLabel = computed(() => {
  return cookieAppOptions.find(option => option.value === cookieAppType.value)?.label || cookieAppType.value;
});

const openCookieModal = () => {
  const savedAppType = p115Info.value?.cookie_app_type;
  if (cookieAppOptions.some(option => option.value === savedAppType)) {
    cookieAppType.value = savedAppType;
  }
  showCookieModal.value = true;
  tempCookieInput.value = '';
  refreshCookieQrcode();
};

const refreshCookieQrcode = async () => {
  stopCookiePolling();
  const requestVersion = ++cookieQrcodeRequestVersion;
  const selectedAppType = cookieAppType.value;
  cookieQrcodeSessionId.value = '';
  cookieQrcodeStatus.value = 'loading';
  cookieQrcodeLoading.value = true;

  try {
    const res = await axios.get('/api/p115/cookie_qrcode', {
      params: { app: selectedAppType }
    });
    if (requestVersion !== cookieQrcodeRequestVersion || selectedAppType !== cookieAppType.value) return;
    if (res.data && res.data.success) {
      const data = res.data.data || {};
      if (!data.session_id || data.client_type !== selectedAppType) {
        cookieQrcodeStatus.value = 'error';
        message.error('二维码客户端校验失败，请重新获取');
        return;
      }
      cookieQrcodeUrl.value = data.qrcode;
      cookieQrcodeSessionId.value = data.session_id;
      cookieQrcodeStatus.value = 'waiting';
      startCookiePolling(data.session_id, data.client_type);
    } else {
      cookieQrcodeStatus.value = 'error';
      message.error(res.data?.message || '获取二维码失败');
    }
  } catch (e) {
    if (requestVersion !== cookieQrcodeRequestVersion) return;
    cookieQrcodeStatus.value = 'error';
    message.error('获取二维码失败: ' + (e.response?.data?.message || e.message));
  } finally {
    if (requestVersion === cookieQrcodeRequestVersion) {
      cookieQrcodeLoading.value = false;
    }
  }
};

const startCookiePolling = (sessionId, boundClientType) => {
  cookieQrcodePolling.value = setInterval(async () => {
    if (sessionId !== cookieQrcodeSessionId.value) return;
    try {
      const res = await axios.get('/api/p115/cookie_qrcode/status', {
        params: { session_id: sessionId }
      });
      const data = res.data;

      if (data.client_type && data.client_type !== boundClientType) {
        cookieQrcodeStatus.value = 'error';
        message.error('Cookie 客户端与二维码绑定客户端不一致');
        stopCookiePolling();
        return;
      }
      if (data.status === 'success') {
        cookieQrcodeStatus.value = 'success';
        message.success(data.message || 'Cookie 获取成功！');
        stopCookiePolling();
        setTimeout(() => {
          showCookieModal.value = false;
          check115Status(); // 刷新状态显示
        }, 1500);
      } else if (data.status === 'expired') {
        cookieQrcodeStatus.value = 'expired';
        stopCookiePolling();
      } else if (data.status === 'error') {
        cookieQrcodeStatus.value = 'error';
        message.error(data.message || 'Cookie 获取失败');
        stopCookiePolling();
      }
    } catch (e) {
      const data = e.response?.data;
      if (data?.status === 'expired') {
        cookieQrcodeStatus.value = 'expired';
      } else {
        cookieQrcodeStatus.value = 'error';
        message.error(data?.message || '检查 Cookie 二维码状态失败');
      }
      stopCookiePolling();
    }
  }, 2000);
};

const stopCookiePolling = () => {
  if (cookieQrcodePolling.value) {
    clearInterval(cookieQrcodePolling.value);
    cookieQrcodePolling.value = null;
  }
};

const closeCookieModal = () => {
  cookieQrcodeRequestVersion += 1;
  stopCookiePolling();
  cookieQrcodeSessionId.value = '';
  showCookieModal.value = false;
  cookieQrcodeStatus.value = 'idle';
};

const saveManualCookie = async () => {
  if (!tempCookieInput.value.trim()) {
    message.warning('请输入 Cookie');
    return;
  }
  try {
    const res = await axios.post('/api/p115/cookie', {
      cookie: tempCookieInput.value,
      app_type: cookieAppType.value
    });
    if (res.data.success) {
      message.success('手动 Cookie 保存成功');
      showCookieModal.value = false;
      check115Status();
    }
  } catch (e) {
    message.error('保存失败: ' + (e.response?.data?.message || e.message));
  }
};

const handleClear115Cookie = async () => {
  clearing115Cookie.value = true;
  try {
    const res = await axios.delete('/api/p115/cookie');
    message.success(res.data?.message || '115 Cookie 已清理');
    closeCookieModal();
    await check115Status(false);
  } catch (e) {
    message.error('清理失败: ' + (e.response?.data?.message || e.message));
  } finally {
    clearing115Cookie.value = false;
  }
};

const openApiAuthLabel = computed(() => {
  if (configModel.value?.p115_auth_method === 'qrcode') {
    return '自定义 AppID';
  }
  return '网页登录授权';
});

const cookieAppLabel = computed(() => {
  return p115Info.value?.cookie_app_label || '未配置';
});

// --- 本地目录浏览器状态 ---
const showLocalFolderModal = ref(false)
const currentLocalPath = ref('')
const localFolders = ref([])
const loadingLocalFolders = ref(false)
const currentLocalTargetField = ref('')
const isCurrentLocalTargetArray = ref(false)

// 打开本地目录浏览器 (增强版)
const openLocalFolderSelector = (targetField, isArray = false) => {
    currentLocalTargetField.value = targetField
    isCurrentLocalTargetArray.value = isArray

    // 决定初始路径
    let startPath = '/'
    if (!isArray && configModel.value[targetField]) {
        startPath = configModel.value[targetField]
    } else if (isArray && configModel.value[targetField] && configModel.value[targetField].length > 0) {
        // 如果是数组且有值，取最后一个值的父目录作为起点，方便连续添加
        const lastPath = configModel.value[targetField][configModel.value[targetField].length - 1]
        startPath = lastPath.substring(0, lastPath.lastIndexOf('/')) || '/'
    }

    currentLocalPath.value = startPath
    showLocalFolderModal.value = true
    fetchLocalFolders(currentLocalPath.value)
}

// 获取本地目录列表 (保持不变)
const fetchLocalFolders = async (path) => {
    loadingLocalFolders.value = true
    try {
        const res = await axios.get('/api/p115/system/directories', { params: { path } })
        if (res.data.code === 200) {
            localFolders.value = res.data.data
            if (res.data.current_path !== undefined) {
                currentLocalPath.value = res.data.current_path
            }
        } else {
            message.error(res.data.message || '获取目录失败')
        }
    } catch (error) {
        if (error.response && error.response.status === 403) {
            message.error('没有权限访问该目录！')
        } else if (error.response && error.response.status === 404) {
            message.error('目录不存在！')
        } else {
            message.error('请求目录失败: ' + (error.response?.data?.message || error.message))
        }
    } finally {
        loadingLocalFolders.value = false
    }
}

// 点击列表中的文件夹 (保持不变)
const selectLocalFolder = (folder) => {
    fetchLocalFolders(folder.path)
}

// 确认选择 (增强版)
const confirmLocalFolder = () => {
    const field = currentLocalTargetField.value
    const path = currentLocalPath.value

    if (isCurrentLocalTargetArray.value) {
        // 如果是多选数组 (如 monitor_paths)
        if (!configModel.value[field]) {
            configModel.value[field] = []
        }
        // 防止重复添加
        if (!configModel.value[field].includes(path)) {
            configModel.value[field].push(path)
            message.success(`已追加路径: ${path}`)
        } else {
            message.warning('该路径已存在列表中')
        }
    } else {
        // 如果是单选字符串 (如 local_strm_root)
        configModel.value[field] = path
        message.success(`已选择路径: ${path}`)
    }

    showLocalFolderModal.value = false
}

// --- 115 云端目录浏览器状态 ---
const showP115FolderModal = ref(false)
const currentP115Cid = ref('0')
const currentP115Path = ref('')
const p115Folders = ref([])
const loadingP115Folders = ref(false)

const openP115FolderSelector = () => {
    currentP115Cid.value = configModel.value.p115_fingerprint_media_root_cid || '0'
    currentP115Path.value = configModel.value.p115_fingerprint_media_root_path || ''
    showP115FolderModal.value = true
    fetchP115Folders(currentP115Cid.value)
}

const fetchP115Folders = async (cid = '0') => {
    loadingP115Folders.value = true
    try {
        const res = await axios.get('/api/p115/directories', { params: { cid: cid || '0' } })
        if (res.data.code === 200) {
            p115Folders.value = res.data.data || []
            currentP115Cid.value = res.data.current_cid || cid || '0'
            currentP115Path.value = res.data.current_path || ''
        } else {
            message.error(res.data.message || '获取 115 目录失败')
        }
    } catch (error) {
        message.error('请求 115 目录失败: ' + (error.response?.data?.message || error.message))
    } finally {
        loadingP115Folders.value = false
    }
}

const selectP115Folder = (folder) => {
    fetchP115Folders(folder.cid)
}

const confirmP115Folder = () => {
    configModel.value.p115_fingerprint_media_root_cid = currentP115Cid.value || '0'
    configModel.value.p115_fingerprint_media_root_path = currentP115Path.value || ''
    message.success(`已选择 115 目录: ${currentP115Path.value || '根目录'}`)
    showP115FolderModal.value = false
}

// ★★★ 115 扫码授权 Modal 逻辑 ★★★
const showQrcodeModal = ref(false);
const qrcodeUrl = ref('');
const qrcodeStatus = ref('idle'); // idle, waiting, success, expired, error
const qrcodeLoading = ref(false);
const qrcodePolling = ref(null);
const handleOpenQrcodeModal = () => {
  if (!configModel.value.p115_app_id || !configModel.value.p115_app_id.trim()) {
    message.warning('请先填写自定义 AppID 并点击底部保存设置，然后再扫码授权！');
    return;
  }
  openQrcodeModal();
};
const openQrcodeModal = async () => {
  showQrcodeModal.value = true;
  qrcodeStatus.value = 'loading';
  qrcodeLoading.value = true;

  try {
    const res = await axios.post('/api/p115/qrcode');
    if (res.data && res.data.success) {
      qrcodeUrl.value = res.data.data.qrcode;
      qrcodeStatus.value = 'waiting';
      startPolling();
    } else {
      qrcodeStatus.value = 'error';
      message.error(res.data?.message || '获取二维码失败');
    }
  } catch (e) {
    qrcodeStatus.value = 'error';
    message.error('获取二维码失败: ' + (e.response?.data?.message || e.message));
  } finally {
    qrcodeLoading.value = false;
  }
};

const startPolling = () => {
  // 每2秒检查一次二维码状态
  qrcodePolling.value = setInterval(async () => {
    try {
      const res = await axios.get('/api/p115/qrcode/status');
      const data = res.data;

      if (data.status === 'success') {
        qrcodeStatus.value = 'success';
        message.success('登录成功！授权已自动保存');
        stopPolling();
        setTimeout(() => {
          showQrcodeModal.value = false;
          check115Status(); // 刷新状态显示
        }, 1500);
      } else if (data.status === 'expired') {
        qrcodeStatus.value = 'expired';
        message.warning('二维码已过期');
        stopPolling();
      }
      // waiting 状态继续轮询
    } catch (e) {
      console.error('检查二维码状态失败', e);
    }
  }, 2000);
};

const stopPolling = () => {
  if (qrcodePolling.value) {
    clearInterval(qrcodePolling.value);
    qrcodePolling.value = null;
  }
};

const closeQrcodeModal = () => {
  stopPolling();
  showQrcodeModal.value = false;
  qrcodeUrl.value = '';
  qrcodeStatus.value = 'idle';
};

// ★★★ 全自动网页授权 (授权码模式) 逻辑 ★★★
const isWebAuthing = ref(false);
let webAuthPolling = null;

const startWebAuth = () => {
  // 1. 获取当前 ETK 的访问地址 (例如 http://192.168.1.100:5257)
  const etkHost = window.location.origin;
  // 2. 拼接回调地址
  const callbackUrl = `${etkHost}/api/p115/auto_save_auth`;
  // 3. 拼接最终的 Worker 登录地址
  const authUrl = `https://115.55565576.xyz/login?callback_url=${encodeURIComponent(callbackUrl)}`;

  // 4. 弹出一个居中的小窗口供用户登录
  const width = 500;
  const height = 600;
  const left = (window.screen.width / 2) - (width / 2);
  const top = (window.screen.height / 2) - (height / 2);
  window.open(authUrl, '115AuthWindow', `width=${width},height=${height},top=${top},left=${left}`);

  isWebAuthing.value = true;
  message.info('请在新弹出的窗口中完成 115 授权...', { duration: 5000 });

  // 5. 开始后台轮询，检查 Token 是否已经悄悄保存成功了
  webAuthPolling = setInterval(async () => {
    try {
      const res = await axios.get('/api/p115/status');
      if (res.data && res.data.data && res.data.data.has_token) {
        // 发现 Token 已经有了！
        clearInterval(webAuthPolling);
        isWebAuthing.value = false;
        message.success('🎉 网页授权成功！Token 已自动保存。');
        check115Status(); // 刷新界面状态
      }
    } catch (e) {
      // 轮询期间的错误静默忽略
    }
  }, 2000);

  // 6. 设置一个 3 分钟的超时，防止用户关了窗口导致一直转圈
  setTimeout(() => {
    if (isWebAuthing.value) {
      clearInterval(webAuthPolling);
      isWebAuthing.value = false;
      message.warning('授权等待超时，请重试。');
    }
  }, 180000);
};

// 检查 115 状态
const check115Status = async () => {
    loading115Info.value = true;
    try {
        const res = await axios.get('/api/p115/status');
        if (res.data && res.data.data) {
            p115Info.value = res.data.data;
            message.success('115 状态刷新成功！');
        } else {
            p115Info.value = null;
        }
    } catch (e) {
        p115Info.value = null;
        message.error('状态获取失败: ' + (e.response?.data?.message || e.response?.data?.data?.msg || e.message));
    } finally {
        loading115Info.value = false;
    }
};

// 辅助函数：获取规则摘要
const genreOptions = computed(() => {
  const map = new Map();
  [...rawMovieGenres.value, ...rawTvGenres.value].forEach(g => { if (g && g.value) map.set(g.value, g); });
  return Array.from(map.values());
});

const cardFields = {
  basic: ['delay_between_items_sec', 'media_info_retry_interval', 'media_info_retry_attempts', 'subtitle_chinese_default_language', 'min_score_for_review', 'max_actors_to_process', 'max_episode_actors_to_process', 'extract_thumb', 'actor_role_add_prefix', 'remove_actors_without_avatars', 'keyword_to_tags', 'studio_to_chinese'],
  monitor: ['monitor_enabled', 'monitor_paths', 'monitor_exclude_dirs', 'monitor_exclude_refresh_delay', 'monitor_extensions', 'tmdb_image_language_preference', 'monitor_sha1_pc_search'],
  sources: ['tmdb_api_key', 'tmdb_api_base_url', 'douban_enable_online_api', 'api_douban_default_cooldown_seconds', 'douban_cookie'],
  p115Account: ['p115_auth_method', 'p115_app_id', 'p115_api_priority', 'p115_request_interval'],
  fingerprints: ['local_strm_root', 'cd2_media_root', 'p115_fingerprint_media_root_cid', 'p115_fingerprint_media_root_path', 'p115_generate_mediainfo', 'p115_enable_sync_delete'],
  emby: ['emby_server_url', 'emby_public_url', 'emby_api_key', 'emby_webhook_secret', 'emby_webhook_ingest_enabled', 'emby_user_id', 'emby_admin_user', 'emby_admin_pass', 'emby_api_timeout', 'libraries_to_ignore'],
  ai: ['ai_provider', 'ai_api_key', 'ai_model_name', 'ai_base_url', 'ai_request_timeout', 'ai_max_retries', 'ai_translation_batch_size', 'ai_translate_actor_role', 'ai_translate_on_sync', 'ai_translate_title', 'ai_translate_overview', 'ai_translate_episode_overview', 'ai_joke_fallback', 'ai_translation_mode'],
  logs: ['log_rotation_size_mb', 'log_rotation_backup_count'],
  ingest: ['auto_repair_truehd_core_on_ingest', 'auto_fill_language_titles_on_ingest'],
  rename: ['movie_rename_format', 'tv_rename_format'],
};
const savingCard = ref(null);
const saveCard = async (cardKey) => {
  const fields = cardFields[cardKey];
  if (!fields || savingConfig.value || !configModel.value) return;
  try {
    await formRef.value?.validate(undefined, rule => fields.includes(rule.configField));
    const payload = JSON.parse(JSON.stringify(Object.fromEntries(
      fields.map(field => [field, configModel.value[field]])
    )));
    savingCard.value = cardKey;
    const success = await handleSaveConfig(payload);
    if (success) {
      message.success('此卡片设置已保存！');
    } else {
      message.error(configError.value || '配置保存失败，请检查后端日志。');
    }
  } catch (errors) {
    message.error('请检查此卡片中的必填项或错误项！');
  } finally {
    savingCard.value = null;
  }
};
const previewRenameFormats = async () => {
  if (!configModel.value) return;
  isPreviewingRenameFormats.value = true;
  try {
    const response = await axios.post('/api/config/rename_preview', {
      movie_format: configModel.value.movie_rename_format,
      tv_format: configModel.value.tv_rename_format,
    });
    renameFormatPreview.value = { ...(response.data || {}), error: '' };
  } catch (error) {
    renameFormatPreview.value = {
      movie: '',
      tv: '',
      error: error.response?.data?.error || '预览重命名格式失败',
    };
  } finally {
    isPreviewingRenameFormats.value = false;
  }
};
watch(
  () => [configModel.value?.movie_rename_format, configModel.value?.tv_rename_format],
  ([movieFormat, tvFormat]) => {
    if (!movieFormat || !tvFormat) return;
    if (renamePreviewTimer) clearTimeout(renamePreviewTimer);
    renamePreviewTimer = setTimeout(previewRenameFormats, 250);
  },
  { immediate: true },
);
const fetchEmbyLibrariesInternal = async () => {
  if (!configModel.value.emby_server_url || !configModel.value.emby_api_key) {
    availableLibraries.value = [];
    return;
  }
  if (loadingLibraries.value) return;
  loadingLibraries.value = true;
  libraryError.value = null;
  try {
    const response = await axios.get(`/api/emby_libraries`);
    availableLibraries.value = response.data || [];
    if (availableLibraries.value.length === 0) libraryError.value = "获取到的媒体库列表为空。";
  } catch (err) {
    availableLibraries.value = [];
    libraryError.value = `获取 Emby 媒体库失败: ${err.response?.data?.error || err.message}`;
  } finally {
    loadingLibraries.value = false;
  }
};
const showExportModal = async () => {
  try {
    const response = await axios.get('/api/database/tables');
    allDbTables.value = response.data;
    tablesToExport.value = [...response.data];
    exportModalVisible.value = true;
  } catch (error) {
    message.error('无法获取数据库表列表，请检查后端日志。');
  }
};
const handleExport = async () => {
  isExporting.value = true;
  exportModalVisible.value = false;
  try {
    const response = await axios.post('/api/database/export', { tables: tablesToExport.value }, { responseType: 'blob' });
    const contentDisposition = response.headers['content-disposition'];
    let filename = 'database_backup.json';
    if (contentDisposition) {
      const match = contentDisposition.match(/filename="?(.+?)"?$/);
      if (match?.[1]) filename = match[1];
    }
    const blobUrl = window.URL.createObjectURL(new Blob([response.data]));
    const link = document.createElement('a');
    link.href = blobUrl;
    link.setAttribute('download', filename);
    document.body.appendChild(link);
    link.click();
    link.remove();
    window.URL.revokeObjectURL(blobUrl);
    message.success('数据已开始导出下载！');
  } catch (err) {
    message.error('导出数据失败，请查看日志。');
  } finally {
    isExporting.value = false;
  }
};
const selectAllForExport = () => tablesToExport.value = [...allDbTables.value];
const deselectAllForExport = () => tablesToExport.value = [];

const handleCustomImportRequest = async ({ file, onFinish, onError }) => {
  const rawFile = file.file;
  if (!rawFile) {
    message.error("未能获取到文件对象。");
    onError();
    return;
  }

  const msgReactive = message.loading('正在解析备份文件...', { duration: 0 });

  try {
    const formData = new FormData();
    formData.append('file', rawFile);
    const response = await axios.post('/api/database/preview-backup', formData);

    msgReactive.destroy();

    const tables = response.data.tables;
    if (!tables || tables.length === 0) {
      message.error('备份文件有效，但其中不包含任何数据表。');
      onError();
      return;
    }

    fileToImport.value = rawFile;
    tablesInBackupFile.value = tables;
    tablesToImport.value = [...tables];
    importModalVisible.value = true;
    onFinish();

  } catch (error) {
    msgReactive.destroy();
    const errorMsg = error.response?.data?.error || '解析备份文件失败，请检查文件是否有效。';
    message.error(errorMsg);
    onError();
  }
};

const cancelImport = () => {
  importModalVisible.value = false;
  fileToImport.value = null;
};

const confirmImport = () => {
  importModalVisible.value = false;
  startImportProcess();
};

const startImportProcess = () => {
  if (!fileToImport.value) {
    message.error("没有要上传的文件。");
    return;
  }
  isImporting.value = true;
  const msgReactive = message.loading('正在上传并恢复数据...', { duration: 0 });

  const formData = new FormData();
  formData.append('file', fileToImport.value);
  formData.append('tables', tablesToImport.value.join(','));

  axios.post('/api/database/import', formData, {
    headers: { 'Content-Type': 'multipart/form-data' }
  })
  .then(response => {
    msgReactive.destroy();
    message.success(response.data?.message || '恢复任务已成功提交！');
  })
  .catch(error => {
    msgReactive.destroy();
    const errorMsg = error.response?.data?.error || '恢复失败，未知错误。';
    message.error(errorMsg, { duration: 8000 });
  })
  .finally(() => {
    isImporting.value = false;
    fileToImport.value = null;
  });
};

const handleCleanupOfflineMedia = async () => {
  isCleaningOfflineMedia.value = true;
  try {
    const response = await axios.post('/api/actions/cleanup-offline-media');
    message.success(response.data.message || '离线媒体数据清理完成！');
  } catch (error) {
    message.error(error.response?.data?.error || '操作失败，请检查后端日志。');
  } finally {
    isCleaningOfflineMedia.value = false;
  }
};

const selectAllForImport = () => tablesToImport.value = [...tablesInBackupFile.value];
const deselectAllForImport = () => tablesToImport.value = [];

const handleCorrectSequences = async () => {
  isCorrecting.value = true;
  try {
    const response = await axios.post('/api/database/correct-sequences');
    message.success(response.data.message || 'ID计数器校准成功！');
  } catch (error) {
    message.error(error.response?.data?.error || '校准失败，请检查后端日志。');
  } finally {
    isCorrecting.value = false;
  }
};

onMounted(async () => {
  updateViewportState();
  window.addEventListener('resize', updateViewportState);
  componentIsMounted.value = true;
  unwatchGlobal = watch(loadingConfig, (isLoading) => {
    if (!isLoading && componentIsMounted.value && configModel.value) {
      check115Status();
      if (!Array.isArray(configModel.value.libraries_to_ignore)) configModel.value.libraries_to_ignore = [];
      if (!configModel.value.p115_auth_method) configModel.value.p115_auth_method = 'web';
      if (!configModel.value.p115_api_priority) configModel.value.p115_api_priority = 'cookie';
      if (configModel.value.emby_server_url && configModel.value.emby_api_key) {
        fetchEmbyLibrariesInternal();
      }
      if (unwatchGlobal) { unwatchGlobal(); }
    }
  }, { immediate: true });
  unwatchEmbyConfig = watch(() => [configModel.value?.emby_server_url, configModel.value?.emby_api_key], (newValues, oldValues) => {
    if (componentIsMounted.value && oldValues) {
      if (newValues[0] !== oldValues[0] || newValues[1] !== oldValues[1]) {
        fetchEmbyLibrariesInternal();
      }
    }
  });
});
onUnmounted(() => {
  window.removeEventListener('resize', updateViewportState);
  componentIsMounted.value = false;
  if (renamePreviewTimer) clearTimeout(renamePreviewTimer);
  cookieQrcodeRequestVersion += 1;
  stopCookiePolling();
  if (unwatchGlobal) unwatchGlobal();
  if (unwatchEmbyConfig) unwatchEmbyConfig();
});
</script>

<style scoped>
.settings-card-grid {
  grid-template-columns: minmax(0, 1fr) !important;
}
.settings-card-grid > div { grid-column: auto !important; }
.settings-page :deep(.dashboard-card > .n-card__content),
.ai-settings-wrapper {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  column-gap: 24px;
  align-items: start;
}
.settings-page :deep(.dashboard-card > .n-card__content > *),
.ai-settings-wrapper > * { grid-column: 1 / -1; min-width: 0; }
.settings-page :deep(.dashboard-card > .n-card__content > .n-form-item),
.settings-page :deep(.dashboard-card > .n-card__content > div:has(> .n-form-item)),
.ai-settings-wrapper > .n-form-item { grid-column: auto; }
@container (max-width: 899px) {
  .settings-page :deep(.dashboard-card > .n-card__content),
  .ai-settings-wrapper { grid-template-columns: minmax(0, 1fr); }
}
.card-title { display: inline-flex; align-items: center; gap: 8px; }
/* 禁用AI设置时的遮罩效果 */
.ai-settings-wrapper {
  transition: opacity 0.3s ease;
}
.content-disabled {
  opacity: 0.6;
}

/* 翻译引擎标签样式 */
.engine-list {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}
.engine-tag {
  cursor: grab;
}
.engine-tag:active {
  cursor: grabbing;
}
.drag-handle {
  margin-right: 6px;
  vertical-align: -0.15em;
}

/* ★★★ 新增的样式 ★★★ */
.description-text {
  font-size: 0.85em;
  color: var(--n-text-color-3);
  margin: 0;
  line-height: 1.6;
}
.warning-text {
  color: var(--n-warning-color-suppl); /* 使用 Naive UI 的警告色 */
  font-weight: bold;
}
.data-action-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(min(100%, 160px), 1fr));
  gap: 10px;
  align-items: stretch;
}
.data-action-grid .action-button {
  width: 100%;
  min-width: 0;
  justify-content: center;
}
.data-action-grid :deep(.n-upload),
.data-action-grid :deep(.n-upload-trigger) {
  width: 100%;
  min-width: 0;
}
.glass-section {
  background-color: rgba(255, 255, 255, 0.1);
  backdrop-filter: blur(10px);
  border-radius: 8px;
  border: 1px solid rgba(255, 255, 255, 0.2);
}
.info-icon {
  color: var(--n-info-color);
  cursor: help;
  font-size: 16px;
  vertical-align: middle;
}
.rename-preview-panel {
  min-width: 0;
  align-self: stretch;
  padding: 12px 14px;
  border: 1px solid var(--n-border-color);
  border-radius: 6px;
  background: var(--n-color-modal);
}
.rename-preview-title {
  margin-bottom: 8px;
  color: var(--n-text-color-2);
  font-size: 13px;
  font-weight: 600;
}
.rename-preview-tree {
  min-height: 28px;
}
.rename-preview-node {
  display: flex;
  align-items: center;
  gap: 7px;
  min-height: 26px;
  color: var(--n-text-color-2);
  font-family: monospace;
  font-size: 12px;
  overflow-wrap: anywhere;
}
.rename-format-grid {
  display: grid;
  grid-template-columns: minmax(0, 1.15fr) minmax(0, 0.85fr);
  gap: 14px 18px;
  align-items: stretch;
}
.rename-format-form {
  min-width: 0;
}
@media (max-width: 900px) {
  .rename-format-grid {
    grid-template-columns: 1fr;
  }
}
@media (max-width: 768px) {
  .settings-page {
    min-width: 0;
  }

  .settings-tabs :deep(.n-tabs-nav-scroll-content) {
    min-width: max-content;
  }

  .settings-tabs :deep(.n-tabs-tab) {
    padding-left: 10px;
    padding-right: 10px;
    white-space: nowrap;
  }

  :deep(.n-form-item-label) {
    justify-content: flex-start !important;
    text-align: left !important;
    padding: 0 0 6px 0 !important;
    min-height: auto !important;
  }

  :deep(.n-form-item-blank) {
    min-width: 0;
  }

  :deep(.n-input),
  :deep(.n-input-number),
  :deep(.n-select),
  :deep(.n-input-group),
  :deep(.n-checkbox-group) {
    max-width: 100%;
  }

  :deep(.n-input-number),
  :deep(.n-select) {
    width: 100% !important;
  }

  :deep(.n-input-group) {
    display: flex;
    align-items: stretch;
  }

  :deep(.n-input-group .n-button) {
    flex-shrink: 0;
  }

  :deep(.n-card-header) {
    padding: 14px 14px 8px !important;
    flex-wrap: wrap;
    row-gap: 10px;
  }

  :deep(.n-card-header__main),
  :deep(.n-card-header__extra) {
    min-width: 0;
    width: 100%;
  }

  :deep(.n-card-header__extra .n-space) {
    justify-content: flex-start !important;
    flex-wrap: wrap;
  }

  :deep(.n-card__content),
  :deep(.n-card__footer) {
    padding: 14px !important;
  }

  .card-title { display: inline-flex; align-items: center; gap: 8px;
    font-size: 15px;
  }

  .description-text {
    font-size: 12px;
    line-height: 1.7;
  }

  .action-button {
    width: 100%;
    justify-content: center;
  }


}

</style>
