<!-- frontend/src/components/settings/GeneralSettingsPage.vue -->
<template>
  <VContainer fluid class="mp-page settings-page settings-container pa-0">
    <h1 class="sr-only">设置</h1>
    <SettingsSearchResults :query="settingsSearch.query.value" :results="settingsSearch.results.value" :notice="settingsSearch.notice.value" @select="settingsSearch.select" />
    <div class="d-flex ga-3 flex-column">
      <!-- ★★★ 最终修正: v-if, v-else-if, v-else 现在是正确的同级兄弟关系 ★★★ -->
      <div v-if="configModel">
        <VForm ref="formRef" @submit.prevent>
          <VWindow :model-value="activeSettingsTab" class="settings-content-window disable-tab-transition" :touch="false">
            <VWindowItem value="system" eager>
              <VRow class="mp-aligned-grid settings-card-grid settings-grid">
                <VCol cols="12" v-show="settingsSearch.showCard('基础设置')">
                  <SettingCard class="dashboard-card" title="基础设置" subtitle="媒体处理与语言显示">

                    <VCardText class="settings-card-body">
                      <div class="settings-field" data-field="delay_between_items_sec">
                        <VTextField type="number"
                          v-model.number="configModel.delay_between_items_sec"
                          :min="0"
                          :step="0.1"
                          placeholder="例如: 0.5"
                          label="处理项目间的延迟 (秒)"
                        ></VTextField>
                      </div>
                      <div class="settings-field" data-field="media_info_retry_interval">
                        <VTextField type="number"
                          v-model.number="configModel.media_info_retry_interval"
                          :min="5"
                          :max="1800"
                          :step="5"
                          placeholder="例如: 30"
                          label="媒体信息验收等待 (秒)"
                        ></VTextField>
                        <div class="settings-hint">
                          <span class="text-medium-emphasis">
                            每轮单项补提取后等待该时长再验收。
                          </span>
                        </div>
                      </div>
                      <div class="settings-field" data-field="media_info_retry_attempts">
                        <VTextField type="number"
                          v-model.number="configModel.media_info_retry_attempts"
                          :min="0"
                          :max="20"
                          :step="1"
                          placeholder="例如: 3"
                          label="媒体信息验收重试次数"
                        ></VTextField>
                        <div class="settings-hint">
                          <span class="text-medium-emphasis">
                            仅重试仍缺少有效媒体信息的项目；设为 0 时只验收，不补提取。
                          </span>
                        </div>
                      </div>
                      <div class="settings-field" data-field="subtitle_chinese_default_language">
                        <VSelect
                          v-model="configModel.subtitle_chinese_default_language"
                          :items="subtitleChineseDefaultLanguageOptions"
                          item-title="label"
                          item-value="value"
                          :return-object="false"
                          label="中文字幕默认语言"
                        ></VSelect>
                        <div class="settings-hint">
                          <span class="text-medium-emphasis">
                            仅用于媒体信息编辑器中未明确简繁的中文字幕“修正语言
                            (Language)”选项，方便填充标题；保留明确的 chs、cht。
                          </span>
                        </div>
                      </div>
                      <div class="settings-field" data-field="max_actors_to_process">
                        <VTextField type="number"
                          v-model.number="configModel.max_actors_to_process"
                          :min="10"
                          :step="10"
                          placeholder="建议 30-100"
                          label="最大主要演员数"
                        ></VTextField>
                        <div class="settings-hint">
                          <span class="text-medium-emphasis">
                            处理后最终演员表数量，超过会截断，优先保留有头像演员。
                          </span>
                        </div>
                      </div>
                      <div class="settings-field" data-field="max_episode_actors_to_process">
                        <VTextField type="number"
                          v-model.number="configModel.max_episode_actors_to_process"
                          :min="0"
                          :step="1"
                          placeholder="例如: 5"
                          label="最大分集演员数"
                        ></VTextField>
                        <div class="settings-hint">
                          <span class="text-medium-emphasis">
                            每集最多处理的演员数量，0代表不单独处理分集演员。
                          </span>
                        </div>
                      </div>
                      <div class="settings-field" data-field="extract_thumb">
                        <VSwitch v-model="configModel.extract_thumb" label="截取视频图片"></VSwitch>
                        <div class="settings-hint">
                          <span class="text-medium-emphasis">
                            从视频流中截取高清画面作为分集图或背景图。
                          </span>
                        </div>
                      </div>
                      <div class="settings-field" data-field="actor_role_add_prefix">
                        <VSwitch v-model="configModel.actor_role_add_prefix" label="为角色名添加前缀"></VSwitch>
                        <div class="settings-hint">
                          <span class="text-medium-emphasis">角色名前加上“饰 ”或“配 ”。</span>
                        </div>
                      </div>
                      <div class="settings-field" data-field="remove_actors_without_avatars">
                        <VSwitch v-model="configModel.remove_actors_without_avatars" label="移除无头像的演员"></VSwitch>
                        <div class="settings-hint">
                          <span class="text-medium-emphasis">
                            在最终演员表移除那些找不到任何可用头像的演员。
                          </span>
                        </div>
                      </div>
                      <div class="settings-field" data-field="keyword_to_tags">
                        <VSwitch v-model="configModel.keyword_to_tags" label="关键词写入标签"></VSwitch>
                        <div class="settings-hint">
                          <span class="text-medium-emphasis">
                            将映射后的中文关键词写入标签。
                          </span>
                        </div>
                      </div>
                      <div class="settings-field" data-field="studio_to_chinese">
                        <VSwitch v-model="configModel.studio_to_chinese" label="工作室中文化"></VSwitch>
                        <div class="settings-hint">
                          <span class="text-medium-emphasis">将工作室名称转换为中文。</span>
                        </div>
                      </div>
                    </VCardText>
                    <template #actions>
                      <div class="d-flex ga-3 flex-wrap align-center justify-end">
                        <VBtn
                          color="primary"
                          :loading="savingCard === 'basic'"
                          :disabled="savingConfig"
                          aria-label="保存基础设置"
                          @click="saveCard('basic')"
                        >
                          <template #prepend><VIcon :icon="SaveIcon" /></template>
                          保存
                        </VBtn>
                      </div>
                    </template>
                  </SettingCard>
                </VCol>
                <VCol cols="12" v-show="settingsSearch.showCard('媒体服务器')">
                  <SettingCard class="dashboard-card" title="媒体服务器" subtitle="媒体服务器与媒体库">

                    <VCardText class="settings-card-body">
                      <!-- ★★★ 调整点1: 恢复双列，但减小间距 x-gap="12" ★★★ -->
                      <VRow class="settings-grid">
                        <!-- 1. Emby URL (左) -->
                        <!-- ★★★ 调整点2: label-width="100" 覆盖全局的200，让输入框更长、更紧凑 ★★★ -->
                        <VCol cols="12" md="6">
                          <div class="settings-field" data-field="emby_server_url">
                            <VTextField
                              v-model="configModel.emby_server_url"
                              prepend-inner-icon="mdi-server"
                              hint="服务端地址，格式：http(s)://ip:port"
                              persistent-hint
                              placeholder="http(s)://ip:port"
                              label="地址"
                            ></VTextField>
                          </div>
                        </VCol>
                        <!-- 2. 外网访问 URL (右) -->
                        <VCol cols="12" md="6">
                          <div class="settings-field" data-field="emby_public_url">
                            <VTextField
                              v-model="configModel.emby_public_url"
                              prepend-inner-icon="mdi-play-network"
                              hint="跳转播放页面使用的地址，格式：http(s)://domain:port"
                              persistent-hint
                              placeholder="http(s)://domain:port"
                              label="外网播放地址"
                            ></VTextField>
                          </div>
                        </VCol>
                        <!-- 3. API Key (左) -->
                        <VCol cols="12" md="6">
                          <div class="settings-field" data-field="emby_api_key">
                            <VTextField
                              v-model="configModel.emby_api_key"
                              prepend-inner-icon="mdi-key"
                              hint="Emby设置->高级->API密钥中生成的密钥"
                              persistent-hint
                              placeholder="输入 API Key"
                              :type="revealedPasswords['emby_api_key'] ? 'text' : 'password'"
                              :append-inner-icon="
                                revealedPasswords['emby_api_key'] ? 'mdi-eye-off-outline' : 'mdi-eye-outline'
                              "
                              @click:append-inner="
                                revealedPasswords['emby_api_key'] = !revealedPasswords['emby_api_key']
                              "
                              label="API密钥"
                            ></VTextField>
                          </div>
                        </VCol>

                        <VCol cols="12" md="6">
                          <div class="settings-field" data-field="emby_webhook_secret">
                            <VTextField
                              v-model="configModel.emby_webhook_secret"
                              placeholder="用于 /webhook/emby?token=..."
                              :type="revealedPasswords['emby_webhook_secret'] ? 'text' : 'password'"
                              :append-inner-icon="
                                revealedPasswords['emby_webhook_secret'] ? 'mdi-eye-off-outline' : 'mdi-eye-outline'
                              "
                              @click:append-inner="
                                revealedPasswords['emby_webhook_secret'] = !revealedPasswords['emby_webhook_secret']
                              "
                              label="Webhook密钥"
                            ></VTextField>
                          </div>
                        </VCol>
                        <VCol cols="12">
                          <div class="settings-field" data-field="emby_webhook_ingest_enabled">
                            <VSwitch v-model="configModel.emby_webhook_ingest_enabled" label="Webhook入库"></VSwitch>
                            <div class="settings-hint">
                              <span class="text-medium-emphasis">
                                关闭后忽略 Emby 的 item.add 和 library.new
                                入库事件；MoviePilot完成通知、元数据更新和删除通知不受影响。
                              </span>
                            </div>
                          </div>
                        </VCol>
                        <!-- 4. 用户 ID (右) -->
                        <VCol cols="12" md="6">
                          <div class="settings-field" data-field="emby_user_id">
                            <VTextField
                              v-model="configModel.emby_user_id"
                              placeholder="32位用户ID"
                              label="用户ID"
                              :rules="embyUserIdValidation"
                            ></VTextField>
                            <div class="settings-hint">
                              <div v-if="isInvalidUserId" style="color: var(--ui-error-color); font-size: 12px">
                                格式错误！ID应为32位。
                              </div>
                            </div>
                          </div>
                        </VCol>
                        <!-- 分割线 (占满一行) -->
                        <VCol cols="12">
                          <div
                            style="margin: 8px 0; font-size: 0.9em; color: var(--ui-text-muted)"
                            class="settings-divider text-body-2 text-medium-emphasis"
                          >
                            管理员凭证 (选填)
                            <VDivider class="mt-2" />
                          </div>
                        </VCol>
                        <!-- 5. 管理员用户 (左) -->
                        <VCol cols="12" md="6">
                          <div class="settings-field" data-field="emby_admin_user">
                            <VTextField
                              v-model="configModel.emby_admin_user"
                              prepend-inner-icon="mdi-account"
                              hint="登录用户名"
                              persistent-hint
                              placeholder="管理员用户名"
                              label="用户名"
                            ></VTextField>
                          </div>
                        </VCol>
                        <!-- 6. 管理员密码 (右) -->
                        <VCol cols="12" md="6">
                          <div class="settings-field" data-field="emby_admin_pass">
                            <VTextField
                              v-model="configModel.emby_admin_pass"
                              prepend-inner-icon="mdi-lock"
                              hint="登录密码"
                              persistent-hint
                              placeholder="管理员密码"
                              :type="revealedPasswords['emby_admin_pass'] ? 'text' : 'password'"
                              :append-inner-icon="
                                revealedPasswords['emby_admin_pass'] ? 'mdi-eye-off-outline' : 'mdi-eye-outline'
                              "
                              @click:append-inner="
                                revealedPasswords['emby_admin_pass'] = !revealedPasswords['emby_admin_pass']
                              "
                              label="密码"
                            ></VTextField>
                          </div>
                        </VCol>
                        <!-- 7. 超时时间 (占满一行，保持长标签) -->
                        <VCol cols="12">
                          <div class="settings-field" data-field="emby_api_timeout">
                            <VTextField type="number"
                              v-model.number="configModel.emby_api_timeout"
                              :min="15"
                              :step="5"
                              placeholder="建议 30-90"
                              style="width: 100%"
                              label="Emby API 超时时间 (秒)"
                            ></VTextField>
                          </div>
                        </VCol>
                        <VCol cols="12">
                          <div class="settings-field" data-field="libraries_to_ignore">
                            <VAutocomplete v-model="configModel.libraries_to_ignore" :items="availableLibraries" item-title="Name" item-value="Id"
                              label="排除媒体库" chips multiple clearable prepend-inner-icon="mdi-library" :loading="loadingLibraries"
                              :disabled="loadingLibraries || Boolean(libraryError) || !availableLibraries.length"
                              hint="选中的媒体库不会参与处理；不选则处理全部媒体库。" persistent-hint />
                            <VBtn class="mt-2" size="small" variant="text" prepend-icon="mdi-refresh" :loading="loadingLibraries" @click="fetchEmbyLibrariesInternal">刷新媒体库</VBtn>
                            <VAlert v-if="libraryError" type="warning" variant="tonal" density="compact" class="mt-2">{{ libraryError }}</VAlert>
                          </div>
                        </VCol>
                      </VRow>
                    </VCardText>
                    <template #actions>
                      <div class="d-flex ga-3 flex-wrap align-center justify-end">
                        <VBtn
                          color="primary"
                          :loading="savingCard === 'emby'"
                          :disabled="savingConfig"
                          aria-label="保存媒体服务器"
                          @click="saveCard('emby')"
                        >
                          <template #prepend><VIcon :icon="SaveIcon" /></template>
                          保存
                        </VBtn>
                      </div>
                    </template>
                  </SettingCard>
                </VCol>
                <VCol cols="12" v-show="settingsSearch.showCard('AI 增强')">
                  <SettingCard class="dashboard-card" title="AI 增强" subtitle="翻译服务与处理策略">

                    <template #append>
                      <div class="d-flex ga-3 flex-wrap align-center">
                        <VBtn data-field="ai_prompt_templates" data-settings-label="配置提示词" color="info" @click="openPromptModal" variant="outlined" size="x-small">
                          <template #prepend><UiIcon name="language" /></template>
                          配置提示词
                        </VBtn>
                        <VBtn
                          color="primary"
                          @click="testAI"
                          :loading="isTestingAI"
                          :disabled="!configModel.ai_api_key"
                          variant="outlined"
                          size="x-small"
                        >
                          <template #prepend><UiIcon name="link" /></template>
                          测试连接
                        </VBtn>
                      </div>
                    </template>
                    <VCardText class="settings-card-body">
                      <!-- 移除 content-disabled 类，因为不再有总开关控制禁用 -->
                      <div class="ai-settings-wrapper">
                        <!-- 1. 基础配置 (上移，因为它们是前提) -->
                        <div class="settings-field" data-field="ai_provider">
                          <VSelect
                            v-model="configModel.ai_provider"
                            prepend-inner-icon="mdi-robot"
                            hint="选择使用的LLM服务提供商"
                            persistent-hint
                            :items="aiProviderOptions"
                            item-title="label"
                            item-value="value"
                            :return-object="false"
                            label="LLM提供商"
                          ></VSelect>
                        </div>
                        <div class="settings-field" data-field="ai_api_key">
                          <VTextField
                            v-model="configModel.ai_api_key"
                            prepend-inner-icon="mdi-key-variant"
                            hint="LLM服务提供商的API密钥，用于身份验证"
                            persistent-hint
                            placeholder="输入你的 API Key"
                            :type="revealedPasswords['ai_api_key'] ? 'text' : 'password'"
                            :append-inner-icon="
                              revealedPasswords['ai_api_key'] ? 'mdi-eye-off-outline' : 'mdi-eye-outline'
                            "
                            @click:append-inner="revealedPasswords['ai_api_key'] = !revealedPasswords['ai_api_key']"
                            label="LLM API密钥"
                          ></VTextField>
                        </div>
                        <div class="settings-field" data-field="ai_model_name">
                          <VCombobox v-model="configModel.ai_model_name" :items="aiModelSelectOptions" :loading="isFetchingAIModels" clearable
                            item-title="label" item-value="value" :return-object="false" label="LLM模型名称"
                            placeholder="请输入或选择LLM模型名称" hint="指定使用的LLM模型，可刷新模型目录或手动输入。" persistent-hint prepend-inner-icon="mdi-brain">
                            <template #append-inner>
                              <VBtn icon="mdi-refresh" variant="text" size="small" aria-label="刷新LLM模型" :loading="isFetchingAIModels" :disabled="!configModel.ai_api_key" @click="refreshAIModels" />
                            </template>
                          </VCombobox>
                        </div>
                        <div class="settings-field" data-field="ai_base_url">
                          <VTextField
                            v-model="configModel.ai_base_url"
                            prepend-inner-icon="mdi-link"
                            hint="LLM API的基础URL地址，用于自定义API端点"
                            persistent-hint
                            placeholder="用于代理或第三方兼容服务"
                            label="LLM基础URL"
                          ></VTextField>
                        </div>
                        <VRow class="settings-grid">
                          <VCol cols="12" :md="12 / (isMobile ? 1 : 2)">
                            <div class="settings-field" data-field="ai_request_timeout">
                              <VTextField type="number"
                                v-model.number="configModel.ai_request_timeout"
                                :min="30"
                                :max="1800"
                                :step="30"
                                style="width: 100%"
                                label="AI 翻译超时时间（秒）"
                              ></VTextField>
                            </div>
                </VCol>
                          <VCol cols="12" :md="12 / (isMobile ? 1 : 2)">
                            <div
                              v-if="configModel.ai_provider === 'openai'"
                              class="settings-field"
                              data-field="ai_max_retries"
                            >
                              <VTextField type="number"
                                v-model.number="configModel.ai_max_retries"
                                :min="0"
                                  :step="1"
                                style="width: 100%"
                                label="失败重试次数"
                              ></VTextField>
                              <div class="settings-hint">
                                0 表示不重试。超时或临时请求失败后重试当前请求；已成功翻译的词条会使用缓存。
                              </div>
                            </div>
                </VCol>
                          <VCol cols="12" :md="12 / (isMobile ? 1 : 2)">
                            <div class="settings-field" data-field="ai_translation_batch_size">
                              <VTextField type="number"
                                v-model.number="configModel.ai_translation_batch_size"
                                :min="1"
                                :max="100"
                                :step="1"
                                style="width: 100%"
                                label="单批最大翻译词条数"
                              ></VTextField>
                            </div>
                </VCol>
                        </VRow>
                        <div
                          style="margin: 10px 0; font-size: 0.9em; color: var(--ui-text-muted)"
                          class="settings-divider text-body-2 text-medium-emphasis"
                        >
                          功能开关
                          <VDivider class="mt-2" />
                        </div>
                        <!-- 2. 功能细分开关 -->
                        <div class="settings-field">
                          <div class="settings-field-label text-body-2 font-weight-medium mb-2">启用功能</div>
                          <VRow class="settings-grid">
                            <VCol cols="12" :md="12 / (isMobile ? 1 : 2)">
                              <VCheckbox data-field="ai_translate_actor_role" v-model="configModel.ai_translate_actor_role" label="翻译演员与角色" />
                  </VCol>
                            <VCol cols="12" :md="12 / (isMobile ? 1 : 2)">
                              <VCheckbox data-field="ai_translate_on_sync" v-model="configModel.ai_translate_on_sync" label="同步媒体数据时翻译" />
                  </VCol>
                            <VCol cols="12" :md="12 / (isMobile ? 1 : 2)">
                              <VCheckbox data-field="ai_translate_title" v-model="configModel.ai_translate_title" label="翻译片名" />
                  </VCol>
                            <VCol cols="12" :md="12 / (isMobile ? 1 : 2)">
                              <VCheckbox data-field="ai_translate_overview" v-model="configModel.ai_translate_overview" label="翻译简介" />
                  </VCol>
                            <VCol cols="12" :md="12 / (isMobile ? 1 : 2)">
                              <VCheckbox data-field="ai_translate_episode_overview" v-model="configModel.ai_translate_episode_overview" label="翻译分集简介" />
                  </VCol>
                            <VCol cols="12" :md="12 / (isMobile ? 1 : 2)">
                              <VCheckbox data-field="ai_joke_fallback" v-model="configModel.ai_joke_fallback" label="无简介小笑话占位" />
                  </VCol>
                          </VRow>
                        </div>
                        <!-- 3. 高级选项 -->
                        <div
                          v-if="configModel.ai_translate_actor_role || configModel.ai_translate_title_overview"
                          class="settings-field"
                          data-field="ai_translation_mode"
                        >
                          <VRadioGroup
                            v-model="configModel.ai_translation_mode"
                            name="ai_translation_mode"
                            label="翻译模式"
                            inline
                          >
                            <div class="d-flex ga-3 flex-column">
                              <VRadio value="fast"><template #label>快速模式 (仅翻译)</template></VRadio>
                              <VRadio value="quality"><template #label>顾问模式 (结合剧情上下文)</template></VRadio>
                            </div>
                          </VRadioGroup>
                        </div>
                      </div>
                    </VCardText>
                    <template #actions>
                      <div class="d-flex ga-3 flex-wrap align-center justify-end">
                        <VBtn
                          color="primary"
                          :loading="savingCard === 'ai'"
                          :disabled="savingConfig"
                          aria-label="保存AI 增强"
                          @click="saveCard('ai')"
                        >
                          <template #prepend><VIcon :icon="SaveIcon" /></template>
                          保存
                        </VBtn>
                      </div>
                    </template>
                  </SettingCard>
                </VCol>
                <VCol cols="12" v-show="settingsSearch.showCard('日志配置')">
                  <SettingCard class="dashboard-card" title="日志配置" subtitle="日志保留与文件大小">

                    <VCardText class="settings-card-body">
                      <div class="settings-field" data-field="log_rotation_size_mb">
                        <VTextField type="number"
                          v-model.number="configModel.log_rotation_size_mb"
                          :min="1"
                          :step="1"
                          placeholder="例如: 5"
                          label="单个日志文件大小 (MB)"
                        ></VTextField>
                        <div class="settings-hint">
                          <span class="text-medium-emphasis">
                            设置 app.log 文件的最大体积，超限后会轮转。
                          </span>
                        </div>
                      </div>
                      <div class="settings-field" data-field="log_rotation_backup_count">
                        <VTextField type="number"
                          v-model.number="configModel.log_rotation_backup_count"
                          :min="1"
                          :step="1"
                          placeholder="例如: 10"
                          label="日志备份数量"
                        ></VTextField>
                        <div class="settings-hint">
                          <span class="text-medium-emphasis">
                            保留最近的日志文件数量 (app.log.1, app.log.2 ...)。
                          </span>
                        </div>
                      </div>
                    </VCardText>
                    <template #actions>
                      <div class="d-flex ga-3 flex-wrap align-center justify-end">
                        <VBtn
                          color="primary"
                          :loading="savingCard === 'logs'"
                          :disabled="savingConfig"
                          aria-label="保存日志配置"
                          @click="saveCard('logs')"
                        >
                          <template #prepend><VIcon :icon="SaveIcon" /></template>
                          保存
                        </VBtn>
                      </div>
                    </template>
                  </SettingCard>
                </VCol>
                <VCol cols="12" v-show="settingsSearch.showCard('数据管理')">
                  <SettingCard data-field="data_management" data-settings-label="数据管理" class="dashboard-card" title="数据管理" subtitle="备份、还原与缓存维护">

                    <VCardText class="settings-card-body">
                      <div class="d-flex ga-3 flex-column">
                        <div class="data-action-grid">
                          <VBtn @click="showExportModal" :loading="isExporting" class="action-button" color="secondary">
                            <template #prepend><VIcon :icon="ExportIcon" /></template>
                            导出数据
                          </VBtn>
                          <VFileInput
                            :model-value="importFiles"
                            @update:model-value="handleImportFileSelection"
                            accept=".json.gz"
                            label="导入备份文件"
                            prepend-icon="mdi-cloud-upload"
                            hide-details
                            :loading="isPreviewingImport"
                            :disabled="isImporting || isPreviewingImport"
                            class="backup-file-input"
                          />
                          <VBtn
                            @click="showClearTablesModal"
                            :loading="isClearing"
                            class="action-button"
                            color="error"
                            variant="outlined"
                          >
                            <template #prepend><VIcon :icon="ClearIcon" /></template>
                            清空指定表
                          </VBtn>
                          <VBtn
                            color="warning"
                            :loading="isCleaningOfflineMedia"
                            class="action-button"
                            @click="
                              confirmSettingAction(
                                '确定要清理离线媒体数据吗？ 这将删除所有 不在库 的元数据缓存。 此操作用于数据库瘦身，不会影响已入库媒体项。',
                                handleCleanupOfflineMedia,
                                '确认',
                              )
                            "
                            variant="outlined"
                          >
                            <template #prepend><VIcon :icon="ClearIcon" /></template>
                            清理离线媒体
                          </VBtn>
                          <VBtn
                            color="warning"
                            :loading="isCorrecting"
                            class="action-button"
                            @click="
                              confirmSettingAction(
                                '确定要校准所有表的ID自增计数器吗？ 这是一个安全的操作，用于修复导入数据后无法新增条目的问题。',
                                handleCorrectSequences,
                                '确认',
                              )
                            "
                            variant="outlined"
                          >
                            <template #prepend><VIcon :icon="BuildIcon" /></template>
                            校准ID计数器
                          </VBtn>
                          <!-- ### 重置演员映射表 ### -->
                          <VBtn
                            color="warning"
                            :loading="isResettingMappings"
                            class="action-button"
                            @click="showResetMappingsModal"
                            variant="outlined"
                          >
                            <template #prepend><VIcon :icon="SyncIcon" /></template>
                            重置Emby数据
                          </VBtn>
                        </div>
                        <p class="description-text">
                          <b>导出：</b>
                          将数据库中的一个或多个表备份为 JSON.GZ 文件。
                          <br />
                          <b>导入：</b>
                          从 JSON.GZ 备份文件中恢复数据。
                          <br />
                          <b>清空：</b>
                          删除指定表中的所有数据，此操作不可逆。
                          <br />
                          <b>清理离线：</b>
                          移除已不在库的残留元数据记录，给数据库瘦身。
                          <br />
                          <b>校准：</b>
                          修复导入数据可能引起的自增序号错乱的问题。
                          <br />
                          <b>重置：</b>
                          在重建 Emby 媒体库后，使用此功能清空所有旧的 Emby
                          关联数据（用户、播放状态等），并保留核心元数据，以便后续重新扫描和关联。
                        </p>
                      </div>
                    </VCardText>
                  </SettingCard>
                </VCol>
                        </VRow>
            </VWindowItem>
            <VWindowItem value="directory" eager>
              <VRow class="mp-aligned-grid settings-card-grid settings-grid">
                <VCol cols="12" v-show="settingsSearch.showCard('存储')">
                  <SettingCard class="dashboard-card" title="存储" subtitle="115 授权方式与连接状态">

                    <template #append>
                      <div class="d-flex ga-3 flex-wrap align-center">
                        <VBtn
                          size="small"
                          color="success"
                          @click="check115Status(false)"
                          :loading="loading115Info"
                          variant="tonal"
                        >
                          <template #prepend><UiIcon name="refresh" /></template>
                          刷新状态
                        </VBtn>
                      </div>
                    </template>
                    <VCardText class="settings-card-body">
                      <VAlert v-if="p115StatusError" type="error" variant="tonal" class="mb-3">{{ p115StatusError }}</VAlert>
                      <div
                        v-if="p115Info && p115Info.user_info"
                        style="
                          margin-bottom: 16px;
                          padding: 12px;
                          background: rgba(var(--v-theme-on-surface), 0.05);
                          border-radius: 8px;
                          display: flex;
                          align-items: center;
                          gap: 12px;
                        "
                      >
                        <VAvatar :image="p115Info.user_info.user_face_m" size="large"></VAvatar>
                        <div style="flex: 1; overflow: hidden">
                          <div style="font-weight: bold; font-size: 14px; display: flex; align-items: center; gap: 6px">
                            {{ p115Info.user_info.user_name }}
                            <VChip size="x-small" color="warning">
                              {{ p115Info.user_info.vip_info?.level_name || '普通用户' }}
                            </VChip>
                          </div>
                          <div style="font-size: 12px; color: rgba(var(--v-theme-on-surface), 0.68); margin-top: 4px">
                            剩余空间: {{ p115Info.user_info.rt_space_info?.all_remain?.size_format || '未知' }}
                          </div>
                        </div>
                      </div>
                      <div class="settings-field" data-field="p115_auth_method">
                        <VRadioGroup
                          v-model="configModel.p115_auth_method"
                          name="auth_method_group"
                          label="授权方式"
                          inline
                        >
                          <div class="d-flex ga-3 flex-wrap align-center">
                            <VRadio value="web"><template #label>网页登录授权</template></VRadio>
                            <VRadio value="qrcode"><template #label>自定义 AppID 扫码</template></VRadio>
                          </div>
                        </VRadioGroup>
                      </div>
                      <!-- 方式一：网页登录授权 -->
                      <div
                        v-if="configModel.p115_auth_method === 'web' || !configModel.p115_auth_method"
                        class="settings-field"
                      >
                        <div class="settings-field-label text-body-2 font-weight-medium mb-2">OpenAPI</div>
                        <div style="width: 100%" class="d-flex ga-3 flex-column">
                          <div class="d-flex ga-3 flex-wrap align-center justify-space-between">
                            <VChip size="small" :color="p115Info?.has_token ? 'success' : 'secondary'">
                              <template #prepend>
                                <VIcon :icon="p115Info?.has_token ? CheckIcon : CloseIcon" />
                              </template>
                              {{ p115Info?.has_token ? `${openApiAuthLabel}` : `未授权：${openApiAuthLabel}` }}
                            </VChip>
                            <VBtn size="small" color="warning" @click="startWebAuth" :loading="isWebAuthing">
                              <template #prepend><UiIcon name="key" /></template>
                              {{ p115Info?.has_token ? '重新登录' : '登录授权' }}
                            </VBtn>
                          </div>
                        </div>
                      </div>
                      <!-- 方式二：自定义 AppID 扫码 -->
                      <div
                        v-if="configModel.p115_auth_method === 'qrcode'"
                        class="settings-field"
                        data-field="p115_app_id"
                      >
                        <div style="width: 100%" class="d-flex ga-3 flex-column">
                          <div class="d-flex ga-3 flex-wrap align-center justify-space-between">
                            <VChip size="small" :color="p115Info?.has_token ? 'success' : 'secondary'">
                              <template #prepend>
                                <VIcon :icon="p115Info?.has_token ? CheckIcon : CloseIcon" />
                              </template>
                              {{ p115Info?.has_token ? `${openApiAuthLabel}` : `未授权：${openApiAuthLabel}` }}
                            </VChip>
                            <VBtn size="small" color="primary" @click="handleOpenQrcodeModal">
                              <template #prepend><UiIcon name="key" /></template>
                              {{ p115Info?.has_token ? '重新扫码' : '扫码授权' }}
                            </VBtn>
                          </div>
                          <div class="settings-input-group d-flex align-start ga-2">
                            <VTextField
                              v-model="configModel.p115_app_id"
                              placeholder="先保存此卡片的自定义 AppID 再扫码"
                              label="OpenAPI"
                            ></VTextField>
                          </div>
                          <div class="settings-hint">
                            <span class="text-medium-emphasis">
                              请先填写 AppID 并保存此卡片，然后再扫码授权。
                            </span>
                          </div>
                        </div>
                      </div>
                      <!--  Cookie  -->
                      <div class="settings-field">
                        <div class="settings-field-label text-body-2 font-weight-medium mb-2">Cookie</div>
                        <div style="width: 100%" class="d-flex ga-3 flex-column">
                          <div class="d-flex ga-3 flex-wrap align-center justify-space-between">
                            <VChip
                              size="small"
                              :color="
                                !p115Info?.has_cookie
                                  ? 'secondary'
                                  : p115Info?.cookie_valid === false
                                    ? 'error'
                                    : 'success'
                              "
                            >
                              <template #prepend>
                                <VIcon :icon="p115Info?.has_cookie ? CheckIcon : CloseIcon" />
                              </template>
                              {{ p115Info?.has_cookie ? cookieAppLabel : '未配置' }}
                            </VChip>
                            <div class="d-flex ga-3 flex-wrap align-center">
                              <VBtn size="small" color="primary" @click="openCookieModal">
                                <template #prepend><UiIcon name="refresh" /></template>
                                {{ p115Info?.has_cookie ? '重新获取' : '扫码获取' }}
                              </VBtn>
                              <VBtn
                                size="small"
                                color="error"
                                :loading="clearing115Cookie"
                                @click="
                                  confirmSettingAction(
                                    '清理后将保留 OpenAPI Token，但 Cookie 直链缓存会同时失效。确定继续吗？',
                                    handleClear115Cookie,
                                    '清理',
                                  )
                                "
                                v-if="p115Info?.has_cookie"
                                variant="tonal"
                              >
                                <template #prepend><VIcon :icon="ClearIcon" /></template>
                                清理 Cookie
                              </VBtn>
                            </div>
                          </div>
                        </div>
                      </div>
                      <div class="settings-field" data-field="p115_api_priority">
                        <VRadioGroup
                          v-model="configModel.p115_api_priority"
                          name="api_priority_group"
                          label="115接口"
                          inline
                        >
                          <div class="d-flex ga-3 flex-wrap align-center">
                            <VRadio value="cookie"><template #label>优先 Cookie（默认）</template></VRadio>
                            <VRadio value="openapi"><template #label>优先 OpenAPI（备用）</template></VRadio>
                          </div>
                        </VRadioGroup>
                        <div class="settings-hint">
                          <span class="text-medium-emphasis">
                            默认优先 Cookie，失败后再尝试 OpenAPI。
                          </span>
                        </div>
                      </div>
                      <div class="settings-field" data-field="p115_request_interval">
                        <VTextField type="number"
                          v-model.number="configModel.p115_request_interval"
                          :min="0.1"
                          :step="0.1"
                          placeholder="0.5"
                          label="API 请求间隔 (秒)"
                        ></VTextField>
                        <div class="settings-hint">
                          <span class="text-medium-emphasis">
                            115 官方对 API 调用频率有严格限制，建议保持 0.5 秒以上。
                          </span>
                        </div>
                      </div>
                    </VCardText>
                    <template #actions>
                      <div class="d-flex ga-3 flex-wrap align-center justify-end">
                        <VBtn
                          color="primary"
                          :loading="savingCard === 'p115Account'"
                          :disabled="savingConfig"
                          aria-label="保存存储"
                          @click="saveCard('p115Account')"
                        >
                          <template #prepend><VIcon :icon="SaveIcon" /></template>
                          保存
                        </VBtn>
                      </div>
                    </template>
                  </SettingCard>
                </VCol>
                <VCol cols="12" v-show="settingsSearch.showCard('目录')">
                  <SettingCard class="dashboard-card" title="目录" subtitle="媒体目录与文件身份">

                    <VCardText class="settings-card-body">
                      <div class="settings-field" data-field="emby_metadata_path">
                        <LocalPathField v-model="configModel.emby_metadata_path" label="媒体元数据目录" placeholder="选择工具内的 Emby 元数据目录" hint="选择已挂载的 Emby metadata 文件夹；与 Emby 使用相同路径时可留空。" persistent-hint />
                      </div>

                      <div class="settings-field" data-field="local_strm_root">
                        <div class="settings-input-group d-flex align-start ga-2">
                          <VTextField
                            v-model="configModel.local_strm_root"
                            placeholder="例如: /mnt/media"
                            style="flex: 1"
                            label="本地 STRM 根目录"
                          ></VTextField>
                          <VBtn
                            color="primary"
                            aria-label="选择本地 STRM 根目录"
                            @click="openLocalFolderSelector('local_strm_root')"
                            variant="outlined"
                            icon
                          >
                            <template #default><VIcon :icon="FolderIcon" /></template>
                          </VBtn>
                        </div>
                        <div class="settings-hint">
                          <span class="text-medium-emphasis">
                            用于从 Emby 路径反查 115 缓存位置，辅助补齐 PC/SHA1。
                          </span>
                        </div>
                      </div>
                      <div class="settings-field" data-field="cd2_media_root">
                        <div class="settings-input-group d-flex align-start ga-2">
                          <VTextField
                            v-model="configModel.cd2_media_root"
                            placeholder="例如: /cd2/115open/媒体库"
                            style="flex: 1"
                            label="CD2 媒体库本地根目录"
                          ></VTextField>
                          <VBtn
                            color="primary"
                            aria-label="选择 CD2 媒体库根目录"
                            @click="openLocalFolderSelector('cd2_media_root')"
                            variant="outlined"
                            icon
                          >
                            <template #default><VIcon :icon="FolderIcon" /></template>
                          </VBtn>
                        </div>
                      </div>
                      <div class="settings-field" data-field="p115_fingerprint_media_root_path">
                        <div class="settings-input-group d-flex align-start ga-2">
                          <VTextField
                            v-model="configModel.p115_fingerprint_media_root_path"
                            placeholder="例如: 媒体库"
                            readonly
                            style="flex: 1"
                            label="115 云端媒体库根目录"
                          ></VTextField>
                          <VBtn
                            color="primary"
                            aria-label="选择 115 云端目录"
                            @click="openP115FolderSelector"
                            variant="outlined"
                            icon
                          >
                            <template #default><VIcon :icon="FolderIcon" /></template>
                          </VBtn>
                        </div>
                      </div>
                      <div class="settings-field" data-field="p115_generate_mediainfo">
                        <VSwitch v-model="configModel.p115_generate_mediainfo" label="媒体信息格式化"></VSwitch>
                        <div class="settings-hint">
                          <span class="text-medium-emphasis">
                            Webhook 或指纹任务可将媒体信息写入本地缓存，用于备份与还原。
                          </span>
                        </div>
                      </div>
                      <div class="settings-field" data-field="p115_enable_sync_delete">
                        <VSwitch v-model="configModel.p115_enable_sync_delete" label="Emby 删除联动 115 删除"></VSwitch>
                        <div class="settings-hint">
                          <span class="text-medium-emphasis">
                            通知到达时同步删除匹配的 115 文件。此操作不可逆，请确认删除链路可靠后再开启。
                          </span>
                        </div>
                      </div>
                    </VCardText>
                    <template #actions>
                      <div class="d-flex ga-3 flex-wrap align-center justify-end">
                        <VBtn
                          color="primary"
                          :loading="savingCard === 'fingerprints'"
                          :disabled="savingConfig"
                          aria-label="保存目录"
                          @click="saveCard('fingerprints')"
                        >
                          <template #prepend><VIcon :icon="SaveIcon" /></template>
                          保存
                        </VBtn>
                      </div>
                    </template>
                  </SettingCard>
                </VCol>
                        </VRow>
            </VWindowItem>
            <VWindowItem value="rule" eager>
              <VRow class="mp-aligned-grid settings-card-grid settings-grid">
                <VCol cols="12" v-show="settingsSearch.showCard('实时监控')">
                  <SettingCard class="dashboard-card" title="实时监控" subtitle="自动发现本地媒体变化">

                    <VCardText class="settings-card-body">
                      <div class="settings-field" data-field="monitor_enabled">
                        <VSwitch v-model="configModel.monitor_enabled" label="启用文件系统监控"></VSwitch>
                      </div>
                      <div class="settings-field" data-field="monitor_paths">
                        <div class="settings-input-group d-flex align-start ga-2">
                          <VCombobox
                            v-model="configModel.monitor_paths"
                            multiple
                            placeholder="输入路径并回车，或点击右侧选择"
                            :items="[]"
                            style="flex: 1"
                            item-title="label"
                            item-value="value"
                            :return-object="false"
                            chips
                            closable-chips
                            label="监控路径"
                          ></VCombobox>
                          <VBtn
                            color="primary"
                            aria-label="选择监控路径"
                            @click="openLocalFolderSelector('monitor_paths', true)"
                            variant="outlined"
                            icon
                          >
                            <template #default><VIcon :icon="FolderIcon" /></template>
                          </VBtn>
                        </div>
                        <div class="settings-hint">
                          <span class="text-medium-emphasis">
                            请保持和 Emby 媒体库路径映射一致。
                          </span>
                        </div>
                      </div>
                      <div class="settings-field" data-field="monitor_exclude_dirs">
                        <div class="settings-input-group d-flex align-start ga-2">
                          <VCombobox
                            v-model="configModel.monitor_exclude_dirs"
                            multiple
                            placeholder="输入路径并回车，或点击右侧选择"
                            :items="[]"
                            style="flex: 1"
                            item-title="label"
                            item-value="value"
                            :return-object="false"
                            chips
                            closable-chips
                            label="排除路径"
                          ></VCombobox>
                          <VBtn
                            color="primary"
                            aria-label="选择排除路径"
                            @click="openLocalFolderSelector('monitor_exclude_dirs', true)"
                            variant="outlined"
                            icon
                          >
                            <template #default><VIcon :icon="FolderIcon" /></template>
                          </VBtn>
                        </div>
                        <div class="settings-hint">
                          <span class="text-medium-emphasis">
                            命中这些路径的文件将
                            <b>跳过刮削流程</b>
                            ，仅刷新。
                            <br />
                          </span>
                        </div>
                      </div>
                      <!-- 排除刷新延迟 -->
                      <div class="settings-field" data-field="monitor_exclude_refresh_delay">
                        <VTextField type="number"
                          v-model.number="configModel.monitor_exclude_refresh_delay"
                          :min="0"
                          :step="10"
                          placeholder="0"
                          style="width: 100%"
                          label="排除刷新延迟"
                        >
                          <template #append-inner>秒</template>
                        </VTextField>
                        <div class="settings-hint">
                          <span class="text-medium-emphasis">
                            仅对
                            <b>排除路径</b>
                            生效。设为 0 则立即刷新。
                            <br />
                          </span>
                        </div>
                      </div>
                      <div class="settings-field" data-field="monitor_extensions">
                        <VCombobox
                          v-model="configModel.monitor_extensions"
                          multiple
                          placeholder="输入扩展名并回车"
                          :items="monitorExtensionOptions"
                          :return-object="false"
                          chips
                          closable-chips
                          label="监控扩展名"
                        ></VCombobox>

                        <div class="settings-hint">
                          <span class="text-medium-emphasis">
                            选择常见媒体格式，也可以输入扩展名并回车添加。
                          </span>
                        </div>
                      </div>
                      <div class="settings-field" data-field="tmdb_image_language_preference">
                        <VRadioGroup
                          v-model="configModel.tmdb_image_language_preference"
                          name="image_lang_group"
                          label="图片语言偏好"
                          inline
                        >
                          <div class="d-flex ga-3 flex-wrap align-center">
                            <VRadio value="zh"><template #label>简体中文优先</template></VRadio>
                            <VRadio value="original"><template #label>原语言优先</template></VRadio>
                          </div>
                        </VRadioGroup>
                        <div class="settings-hint">
                          <span class="text-medium-emphasis">
                            控制下载 海报 等图片时的语言优先级。
                          </span>
                        </div>
                      </div>
                      <div class="settings-field" data-field="monitor_sha1_pc_search">
                        <VSwitch v-model="configModel.monitor_sha1_pc_search" label="SHA1检测"></VSwitch>
                        <div class="settings-hint">
                          <span class="text-medium-emphasis">
                            文件SHA1查询，非115网盘勿启用。
                          </span>
                        </div>
                      </div>
                    </VCardText>
                    <template #actions>
                      <div class="d-flex ga-3 flex-wrap align-center justify-end">
                        <VBtn
                          color="primary"
                          :loading="savingCard === 'monitor'"
                          :disabled="savingConfig"
                          aria-label="保存实时监控"
                          @click="saveCard('monitor')"
                        >
                          <template #prepend><VIcon :icon="SaveIcon" /></template>
                          保存
                        </VBtn>
                      </div>
                    </template>
                  </SettingCard>
                </VCol>
                <VCol cols="12" v-show="settingsSearch.showCard('入库自动处理')">
                  <SettingCard class="dashboard-card" title="入库自动处理" subtitle="新媒体入库后的增强任务">

                    <VCardText class="settings-card-body">
                      <div class="settings-field" data-field="auto_repair_truehd_core_on_ingest">
                        <VSwitch
                          v-model="configModel.auto_repair_truehd_core_on_ingest"
                          label="入库自动纠正高清音轨"
                        ></VSwitch>
                        <div class="settings-hint">
                          <span class="text-medium-emphasis">
                            仅在入库时执行安全的兼容核心纠正；手动音轨纠正任务不受此开关影响。默认关闭。
                          </span>
                        </div>
                      </div>
                      <div class="settings-field" data-field="auto_fill_language_titles_on_ingest">
                        <VSwitch
                          v-model="configModel.auto_fill_language_titles_on_ingest"
                          label="入库自动填充语言和标题"
                        ></VSwitch>
                        <div class="settings-hint">
                          <span class="text-medium-emphasis">
                            根据原始标题识别语言，填充音轨和非中文字幕的语言及标题；中文字幕正常跳过并保留原样，未知语言需手动确认。默认关闭。
                          </span>
                        </div>
                      </div>
                      <div class="settings-field" data-field="auto_extract_intro_fingerprint_on_ingest">
                        <VSwitch
                          v-model="configModel.auto_extract_intro_fingerprint_on_ingest"
                          label="入库自动提取片头声纹"
                        ></VSwitch>
                        <div class="settings-hint">
                          <span class="text-medium-emphasis">
                            电视剧入库且标题填充、回读验收完成后自动提取片头声纹，电影不处理。标题未完成时跳过。默认关闭。
                          </span>
                        </div>
                      </div>
                      <div class="settings-field" data-field="auto_extract_video_thumbnails_on_ingest">
                        <VSwitch
                          v-model="configModel.auto_extract_video_thumbnails_on_ingest"
                          label="入库自动提取视频预览缩略图"
                        ></VSwitch>
                        <div class="settings-hint">
                          <span class="text-medium-emphasis">
                            电视剧和电影入库且标题填充、回读验收完成后生成视频预览缩略图，失败时自动尝试修复。标题未完成时跳过。默认关闭。
                          </span>
                        </div>
                      </div>
                      <div class="settings-field" data-field="extract_video_thumbnails_after_save">
                        <VSwitch
                          v-model="configModel.extract_video_thumbnails_after_save"
                          label="保存后提取视频缩略图"
                        ></VSwitch>
                        <div class="settings-hint">
                          <span class="text-medium-emphasis">
                            手动保存媒体信息后运行神医视频缩略图任务，按神医当前配置范围处理。默认关闭。
                          </span>
                        </div>
                      </div>
                    </VCardText>
                    <template #actions>
                      <div class="d-flex ga-3 flex-wrap align-center justify-end">
                        <VBtn
                          color="primary"
                          :loading="savingCard === 'ingest'"
                          :disabled="savingConfig"
                          aria-label="保存入库自动处理"
                          @click="saveCard('ingest')"
                        >
                          <template #prepend><VIcon :icon="SaveIcon" /></template>
                          保存
                        </VBtn>
                      </div>
                    </template>
                  </SettingCard>
                </VCol>
                <VCol cols="12" v-show="settingsSearch.showCard('整理 & 刮削')">
                  <SettingCard class="dashboard-card" title="整理 & 刮削" subtitle="电影与电视剧的目录和文件名">

                    <VCardText class="settings-card-body">
                      <div class="rename-format-grid">
                        <div class="rename-format-form">
                          <div class="settings-field" data-field="movie_rename_format">
                            <VTextarea
                              v-model="configModel.movie_rename_format"
                              spellcheck="false"
                              auto-grow
                              rows="3"
                              max-rows="8"
                              label="电影重命名格式"
                            ></VTextarea>
                            <div class="settings-hint">
                              <span class="text-medium-emphasis">
                                与 MoviePilot「存储&目录 → 整理&刮削」使用相同的 Jinja2
                                格式；扩展名始终使用源文件扩展名。
                              </span>
                            </div>
                          </div>
                        </div>
                        <div class="rename-preview-panel">
                          <div class="rename-preview-title">电影示例<span v-if="renameFormatPreview.movie_sample?.title"> · {{ renameFormatPreview.movie_sample.title }}</span></div>
                          <div class="settings-loading">
                            <VProgressLinear
                              v-if="isPreviewingRenameFormats"
                              indeterminate
                              color="primary"
                              class="mb-3"
                            />
                            <div v-if="movieRenamePreviewParts.length" class="rename-preview-tree">
                              <div
                                v-for="(part, index) in movieRenamePreviewParts"
                                :key="`${part}-${index}`"
                                class="rename-preview-node"
                                :style="{ paddingLeft: `${index * 22}px` }"
                              >
                                <VIcon
                                  :icon="index === movieRenamePreviewParts.length - 1 ? FileIcon : FolderIcon"
                                  :color="index === movieRenamePreviewParts.length - 1 ? 'info' : 'warning'"
                                />
                                <span>{{ part }}</span>
                              </div>
                            </div>
                            <VAlert v-else :type="renameFormatPreview.errors?.movie ? 'error' : 'info'" variant="tonal">
                              {{ renameFormatPreview.errors?.movie || (isPreviewingRenameFormats ? '正在生成预览...' : renameFormatPreview.movie_sample ? '请输入电影重命名格式' : '暂无最近入库的电影') }}
                            </VAlert>
                          </div>
                        </div>
                        <div class="rename-format-form">
                          <div class="settings-field" data-field="tv_rename_format">
                            <VTextarea
                              v-model="configModel.tv_rename_format"
                              spellcheck="false"
                              auto-grow
                              rows="3"
                              max-rows="8"
                              label="电视剧重命名格式"
                            ></VTextarea>
                            <div class="settings-hint">
                              <span class="text-medium-emphasis">
                                与 MoviePilot「存储&目录 → 整理&刮削」使用相同的 Jinja2 格式；可使用 `season`、`episode`
                                和 `season_episode`。
                              </span>
                            </div>
                          </div>
                        </div>
                        <div class="rename-preview-panel">
                          <div class="rename-preview-title">电视剧示例<span v-if="renameFormatPreview.tv_sample?.title"> · {{ renameFormatPreview.tv_sample.title }} {{ renameFormatPreview.tv_sample.season_episode }}</span></div>
                          <div class="settings-loading">
                            <VProgressLinear
                              v-if="isPreviewingRenameFormats"
                              indeterminate
                              color="primary"
                              class="mb-3"
                            />
                            <div v-if="tvRenamePreviewParts.length" class="rename-preview-tree">
                              <div
                                v-for="(part, index) in tvRenamePreviewParts"
                                :key="`${part}-${index}`"
                                class="rename-preview-node"
                                :style="{ paddingLeft: `${index * 22}px` }"
                              >
                                <VIcon
                                  :icon="index === tvRenamePreviewParts.length - 1 ? FileIcon : FolderIcon"
                                  :color="index === tvRenamePreviewParts.length - 1 ? 'info' : 'warning'"
                                />
                                <span>{{ part }}</span>
                              </div>
                            </div>
                            <VAlert v-else :type="renameFormatPreview.errors?.tv ? 'error' : 'info'" variant="tonal">
                              {{ renameFormatPreview.errors?.tv || (isPreviewingRenameFormats ? '正在生成预览...' : renameFormatPreview.tv_sample ? '请输入电视剧重命名格式' : '暂无最近入库的剧集') }}
                            </VAlert>
                          </div>
                        </div>
                      </div>
                    </VCardText>
                    <template #actions>
                      <div class="d-flex ga-3 flex-wrap align-center justify-end">
                        <VBtn
                          color="primary"
                          :loading="savingCard === 'rename'"
                          :disabled="savingConfig"
                          aria-label="保存整理与刮削"
                          @click="saveCard('rename')"
                        >
                          <template #prepend><VIcon :icon="SaveIcon" /></template>
                          保存
                        </VBtn>
                      </div>
                    </template>
                  </SettingCard>
                </VCol>
                        </VRow>
            </VWindowItem>
            <VWindowItem value="search" eager>
              <VRow class="mp-aligned-grid settings-card-grid settings-grid">
                <VCol cols="12" v-show="settingsSearch.showCard('数据源与API')">
                  <SettingCard class="dashboard-card" title="数据源与API" subtitle="元数据服务与访问凭据">

                    <VCardText class="settings-card-body">
                      <div class="settings-field" data-field="tmdb_api_key">
                        <VTextField
                          v-model="configModel.tmdb_api_key"
                          prepend-inner-icon="mdi-key"
                          hint="设置 TheMovieDb API Key"
                          persistent-hint
                          placeholder="输入你的 TMDB API Key"
                          :type="revealedPasswords['tmdb_api_key'] ? 'text' : 'password'"
                          :append-inner-icon="
                            revealedPasswords['tmdb_api_key'] ? 'mdi-eye-off-outline' : 'mdi-eye-outline'
                          "
                          @click:append-inner="revealedPasswords['tmdb_api_key'] = !revealedPasswords['tmdb_api_key']"
                          label="TMDB API Key"
                        ></VTextField>
                      </div>
                      <div class="settings-field" data-field="tmdb_api_base_url">
                        <VTextField
                          v-model="configModel.tmdb_api_base_url"
                          prepend-inner-icon="mdi-api"
                          hint="自定义 TheMovieDb API域名或代理地址"
                          persistent-hint
                          placeholder="https://api.themoviedb.org/3"
                          label="TMDB API服务地址"
                        ></VTextField>
                        <div class="settings-hint">
                          <span class="text-medium-emphasis">
                            TMDb API的基础URL，通常不需要修改。
                          </span>
                        </div>
                      </div>
                      <div class="settings-field" data-field="douban_enable_online_api">
                        <div class="d-flex ga-3 flex-wrap align-center">
                          <VSwitch v-model="configModel.douban_enable_online_api" label="启用豆瓣辅助"></VSwitch>
                          <span style="font-size: 0.9em; margin-left: 8px" class="text-medium-emphasis">
                            从豆瓣补充缺失的演员以及部分中文角色名。
                          </span>
                        </div>
                      </div>
                      <div class="settings-field" data-field="api_douban_default_cooldown_seconds">
                        <VTextField type="number"
                          v-model.number="configModel.api_douban_default_cooldown_seconds"
                          :min="0.1"
                          :step="0.1"
                          placeholder="例如: 1.0"
                          label="豆瓣API冷却时间 (秒)"
                        ></VTextField>
                      </div>
                      <div class="settings-field" data-field="douban_cookie">
                        <VTextField
                          v-model="configModel.douban_cookie"
                          placeholder="从浏览器开发者工具中获取"
                          :type="revealedPasswords['douban_cookie'] ? 'text' : 'password'"
                          :append-inner-icon="
                            revealedPasswords['douban_cookie'] ? 'mdi-eye-off-outline' : 'mdi-eye-outline'
                          "
                          @click:append-inner="revealedPasswords['douban_cookie'] = !revealedPasswords['douban_cookie']"
                          label="豆瓣登录 Cookie"
                        ></VTextField>
                        <div class="settings-hint">
                          <span class="text-medium-emphasis">非必要不用配置，作用有限。</span>
                        </div>
                      </div>
                    </VCardText>
                    <template #actions>
                      <div class="d-flex ga-3 flex-wrap align-center justify-end">
                        <VBtn
                          color="primary"
                          :loading="savingCard === 'sources'"
                          :disabled="savingConfig"
                          aria-label="保存数据源与API"
                          @click="saveCard('sources')"
                        >
                          <template #prepend><VIcon :icon="SaveIcon" /></template>
                          保存
                        </VBtn>
                      </div>
                    </template>
                  </SettingCard>
                </VCol>
                        </VRow>
            </VWindowItem>
                    </VWindow>
        </VForm>
      </div>
      <VAlert v-else-if="configError" title="加载配置失败" type="error" variant="tonal">{{ configError }}</VAlert>
      <div v-else>正在加载配置...</div>
    </div>
    <!-- ★★★ Cookie 扫码获取弹窗 ★★★ -->
    <VDialog
      v-model="showCookieModal"
      :fullscreen="isMobile"
      max-width="400"
      width="calc(100vw - 24px)"
      persistent
      @update:model-value="$event || closeCookieModal()"
    >
      <VCard class="settings-dialog">
        <VCardTitle class="d-flex align-center justify-space-between ga-2">
          <span class="text-h6">扫码获取 Cookie</span>
          <VBtn
            icon="mdi-close" color="default"
            variant="text"
            size="small"
            aria-label="关闭扫码获取 Cookie"
            @click="closeCookieModal()"
          />
        </VCardTitle>
        <VCardText>
          <div class="d-flex ga-3 flex-column">
            <VAlert type="info" style="margin-bottom: 10px" variant="tonal">
              本次二维码会绑定所选客户端，获取 Cookie 后还会校验实际客户端是否一致。
            </VAlert>
            <div class="d-flex ga-3 flex-wrap align-center">
              <VSelect
                v-model="cookieAppType"
                :items="cookieAppOptions"
                style="width: 220px"
                @update:model-value="refreshCookieQrcode"
                item-title="label"
                item-value="value"
                :return-object="false"
              ></VSelect>
              <VBtn color="primary" @click="refreshCookieQrcode" :loading="cookieQrcodeLoading">
                <template #prepend><UiIcon name="refresh" /></template>
                刷新二维码
              </VBtn>
            </div>
            <div style="text-align: center; padding: 20px 0; min-height: 250px">
              <div v-if="cookieQrcodeStatus === 'loading'" class="settings-loading">
                <VProgressCircular indeterminate color="primary" class="my-4" />
                <span class="text-body-2 text-medium-emphasis">正在获取二维码...</span>
              </div>
              <div v-else-if="cookieQrcodeStatus === 'waiting' || cookieQrcodeStatus === 'success'">
                <SettingsQrCode
                  v-if="cookieQrcodeUrl"
                  :value="cookieQrcodeUrl"
                  :size="200"
                  style="margin: 0 auto 20px"
                />
                <VAlert v-if="cookieQrcodeStatus === 'waiting'" type="info" variant="tonal">
                  请使用 {{ selectedCookieAppLabel }} 扫描二维码并确认登录
                </VAlert>
                <VAlert v-if="cookieQrcodeStatus === 'success'" type="success" variant="tonal">
                  获取成功！Cookie 已自动保存到数据库
                </VAlert>
              </div>
              <div v-else-if="cookieQrcodeStatus === 'expired'">
                <VAlert title="二维码已过期" type="warning" variant="tonal">
                  <div class="d-flex justify-center ga-2 mt-4">
                    <VBtn color="primary" @click="refreshCookieQrcode">
                      <template #prepend><UiIcon name="refresh" /></template>
                      重新获取
                    </VBtn>
                  </div>
                </VAlert>
              </div>
              <div v-else-if="cookieQrcodeStatus === 'error'">
                <VAlert title="Cookie 获取失败" type="error" variant="tonal">
                  <div class="d-flex justify-center ga-2 mt-4">
                    <VBtn color="primary" @click="refreshCookieQrcode">
                      <template #prepend><UiIcon name="refresh" /></template>
                      重新获取
                    </VBtn>
                  </div>
                </VAlert>
              </div>
            </div>
            <VDivider style="margin: 0" />
            <VExpansionPanels variant="accordion">
              <VExpansionPanel title="手动粘贴 Cookie (备用方案)" value="manual">
                <VExpansionPanelText>
                  <VTextarea v-model="tempCookieInput" placeholder="UID=...; CID=...; SEID=..." :rows="3"></VTextarea>
                  <VBtn color="primary" size="small" style="margin-top: 8px" @click="saveManualCookie">
                    <template #prepend><UiIcon name="save" /></template>
                    保存手动输入的 Cookie
                  </VBtn>
                </VExpansionPanelText>
              </VExpansionPanel>
            </VExpansionPanels>
          </div>
        </VCardText>
        <VCardActions class="justify-end flex-wrap ga-2">
          <VBtn @click="closeCookieModal" v-if="cookieQrcodeStatus !== 'success'" color="secondary">关闭</VBtn>
        </VCardActions>
      </VCard>
    </VDialog>
    <!-- ★★★ 115 扫码登录弹窗 ★★★ -->
    <VDialog
      v-model="showQrcodeModal"
      :fullscreen="isMobile"
      max-width="350"
      width="calc(100vw - 24px)"
      persistent
      @update:model-value="$event || closeQrcodeModal()"
    >
      <VCard class="settings-dialog">
        <VCardTitle class="d-flex align-center justify-space-between ga-2">
          <span class="text-h6">115 扫码登录</span>
          <VBtn
            icon="mdi-close" color="default"
            variant="text"
            size="small"
            aria-label="关闭115 扫码登录"
            @click="closeQrcodeModal()"
          />
        </VCardTitle>
        <VCardText>
          <div style="text-align: center; padding: 20px 0">
            <!-- 加载中状态 -->
            <div v-if="qrcodeStatus === 'loading'" class="settings-loading">
              <VProgressCircular indeterminate color="primary" class="my-4" />
              <span class="text-body-2 text-medium-emphasis">正在获取二维码...</span>
            </div>
            <!-- 二维码显示 -->
            <div v-else-if="qrcodeStatus === 'waiting' || qrcodeStatus === 'success'">
              <SettingsQrCode v-if="qrcodeUrl" :value="qrcodeUrl" :size="200" style="margin: 0 auto 20px" />
              <VAlert v-if="qrcodeStatus === 'waiting'" type="info" variant="tonal">
                请使用 115 APP 扫描二维码登录
              </VAlert>
              <VAlert v-if="qrcodeStatus === 'success'" type="success" variant="tonal">
                登录成功！ Cookies 已自动保存
              </VAlert>
            </div>
            <!-- 过期状态 -->
            <div v-else-if="qrcodeStatus === 'expired'">
              <VAlert title="二维码已过期" type="warning" variant="tonal">
                <div class="d-flex justify-center ga-2 mt-4">
                  <VBtn color="primary" @click="openQrcodeModal">
                    <template #prepend><UiIcon name="refresh" /></template>
                    重新获取二维码
                  </VBtn>
                </div>
              </VAlert>
            </div>
            <!-- 错误状态 -->
            <div v-else-if="qrcodeStatus === 'error'">
              <VAlert title="获取二维码失败" type="error" variant="tonal">
                <div class="d-flex justify-center ga-2 mt-4">
                  <VBtn color="primary" @click="openQrcodeModal">
                    <template #prepend><UiIcon name="refresh" /></template>
                    重试
                  </VBtn>
                </div>
              </VAlert>
            </div>
          </div>
        </VCardText>
        <VCardActions class="justify-end flex-wrap ga-2">
          <VBtn @click="closeQrcodeModal" v-if="qrcodeStatus !== 'success'" color="secondary">关闭</VBtn>
        </VCardActions>
      </VCard>
    </VDialog>
    <!-- ★★★ 本地物理目录选择器弹窗 ★★★ -->
    <VDialog
      v-model="showLocalFolderModal"
      :fullscreen="isMobile"
      max-width="600"
      width="calc(100vw - 24px)"
      @update:model-value="$event || (showLocalFolderModal = false)"
    >
      <VCard class="settings-dialog">
        <VCardTitle class="d-flex align-center justify-space-between ga-2">
          <span class="text-h6">选择本地路径</span>
          <VBtn
            icon="mdi-close" color="default"
            variant="text"
            size="small"
            aria-label="关闭选择本地路径"
            @click="showLocalFolderModal = false"
          />
        </VCardTitle>
        <VCardText>
          <div class="settings-loading">
            <VProgressLinear v-if="loadingLocalFolders" indeterminate color="primary" class="mb-3" />
            <div class="d-flex ga-3 flex-column">
              <!-- 顶部路径输入与刷新 -->
              <div class="settings-input-group d-flex align-start ga-2">
                <VTextField
                  v-model="currentLocalPath"
                  placeholder="当前路径"
                  @keyup.enter="fetchLocalFolders(currentLocalPath)"
                ></VTextField>
                <VBtn color="primary" @click="fetchLocalFolders(currentLocalPath)" icon>
                  <template #default><VIcon :icon="RefreshIcon" /></template>
                </VBtn>
              </div>
              <!-- 目录列表 -->
              <VList style="max-height: 400px; overflow-y: auto; border-radius: 6px">
                <VListItem v-for="folder in localFolders" :key="folder.path" @click="selectLocalFolder(folder)">
                  <template #prepend>
                    <VIcon
                      :icon="folder.is_parent ? ArrowUpIcon : FolderIcon"
                      size="22"
                      :color="folder.is_parent ? 'secondary' : 'warning'"
                    />
                  </template>
                  <span :style="{ fontWeight: folder.is_parent ? 'bold' : 'normal' }">{{ folder.name }}</span>
                </VListItem>
                <div
                  v-if="localFolders.length === 0"
                  style="margin-top: 30px; margin-bottom: 30px"
                  class="pa-6 text-center text-medium-emphasis"
                >
                  <VIcon icon="mdi-folder-open-outline" size="36" />
                  <p class="mt-2">空目录或无权限访问</p>
                </div>
              </VList>
              <!-- 底部按钮 -->
              <div style="display: flex; justify-content: flex-end; gap: 12px; margin-top: 16px">
                <VBtn @click="showLocalFolderModal = false" color="secondary">取消</VBtn>
                <VBtn color="primary" @click="confirmLocalFolder" :disabled="loadingLocalFolders || !localFolderSelectionReady">
                  <template #prepend><UiIcon name="select" /></template>
                  确定选择此目录
                </VBtn>
              </div>
            </div>
          </div>
        </VCardText>
      </VCard>
    </VDialog>
    <VDialog
      v-model="showP115FolderModal"
      :fullscreen="isMobile"
      max-width="600"
      width="calc(100vw - 24px)"
      @update:model-value="$event || (showP115FolderModal = false)"
    >
      <VCard class="settings-dialog">
        <VCardTitle class="d-flex align-center justify-space-between ga-2">
          <span class="text-h6">选择 115 云端目录</span>
          <VBtn
            icon="mdi-close" color="default"
            variant="text"
            size="small"
            aria-label="关闭选择 115 云端目录"
            @click="showP115FolderModal = false"
          />
        </VCardTitle>
        <VCardText>
          <div class="settings-loading">
            <VProgressLinear v-if="loadingP115Folders" indeterminate color="primary" class="mb-3" />
            <div class="d-flex ga-3 flex-column">
              <div class="settings-input-group d-flex align-start ga-2">
                <VTextField :model-value="currentP115Path || '根目录'" readonly></VTextField>
                <VBtn color="primary" @click="fetchP115Folders(currentP115Cid)" icon>
                  <template #default><VIcon :icon="RefreshIcon" /></template>
                </VBtn>
              </div>
              <VList style="max-height: 400px; overflow-y: auto; border-radius: 6px">
                <VListItem
                  v-for="folder in p115Folders"
                  :key="`${folder.cid}-${folder.path}`"
                  @click="selectP115Folder(folder)"
                >
                  <template #prepend>
                    <VIcon
                      :icon="folder.is_parent ? ArrowUpIcon : FolderIcon"
                      size="22"
                      :color="folder.is_parent ? 'secondary' : 'warning'"
                    />
                  </template>
                  <span :style="{ fontWeight: folder.is_parent ? 'bold' : 'normal' }">{{ folder.name }}</span>
                </VListItem>
                <div
                  v-if="p115Folders.length === 0"
                  style="margin-top: 30px; margin-bottom: 30px"
                  class="pa-6 text-center text-medium-emphasis"
                >
                  <VIcon icon="mdi-folder-open-outline" size="36" />
                  <p class="mt-2">空目录</p>
                </div>
              </VList>
              <div style="display: flex; justify-content: flex-end; gap: 12px; margin-top: 16px">
                <VBtn @click="showP115FolderModal = false" color="secondary">取消</VBtn>
                <VBtn color="primary" @click="confirmP115Folder" :disabled="loadingP115Folders || !p115FolderSelectionReady">
                  <template #prepend><UiIcon name="select" /></template>
                  确定选择此目录
                </VBtn>
              </div>
            </div>
          </div>
        </VCardText>
      </VCard>
    </VDialog>
  </VContainer>
  <!-- 导出选项模态框 -->
  <VDialog
    v-model="exportModalVisible"
    :fullscreen="isMobile"
    max-width="520"
    width="calc(100vw - 24px)"
    @update:model-value="$event || (exportModalVisible = false)"
  >
    <VCard class="settings-dialog">
      <VCardTitle class="d-flex align-center justify-space-between ga-2">
        <span class="text-h6">选择要导出的数据表</span>
        <VBtn
          icon="mdi-close" color="default"
          variant="text"
          size="small"
          aria-label="关闭选择要导出的数据表"
          @click="exportModalVisible = false"
        />
      </VCardTitle>
      <VCardText>
        <div style="margin-bottom: 10px" class="d-flex ga-3 flex-wrap align-center justify-end">
          <VBtn color="primary" @click="selectAllForExport" variant="text">
            <template #prepend><UiIcon name="select" /></template>
            全选
          </VBtn>
          <VBtn color="primary" @click="deselectAllForExport" variant="text">全不选</VBtn>
        </div>
        <div class="settings-choice-group">
          <VRow class="settings-grid">
            <VCol cols="12" :md="12 / (isMobile ? 1 : 2)" v-for="table in allDbTables" :key="table">
              <VCheckbox :value="table" v-model="tablesToExport" multiple>
                <template #label>{{ tableInfo[table]?.cn || table }}</template>
              </VCheckbox>
            </VCol>
          </VRow>
        </div>
      </VCardText>
      <VCardActions class="justify-end flex-wrap ga-2">
        <VBtn @click="exportModalVisible = false" color="secondary">取消</VBtn>
        <VBtn color="primary" @click="handleExport" :disabled="tablesToExport.length === 0">
          <template #prepend><UiIcon name="download" /></template>
          确认导出
        </VBtn>
      </VCardActions>
    </VCard>
  </VDialog>
  <!-- 导入选项模态框 -->
  <VDialog
    v-model="importModalVisible"
    :fullscreen="isMobile"
    max-width="520"
    width="calc(100vw - 24px)"
    @update:model-value="$event || cancelImport()"
  >
    <VCard class="settings-dialog">
      <VCardTitle class="d-flex align-center justify-space-between ga-2">
        <span class="text-h6">恢复数据库备份</span>
        <VBtn icon="mdi-close" color="default" variant="text" size="small" aria-label="关闭恢复数据库备份" @click="cancelImport()" />
      </VCardTitle>
      <VCardText>
        <div class="d-flex ga-3 flex-column">
          <div>
            <p>
              <strong>文件名:</strong>
              {{ fileToImport?.name }}
            </p>
          </div>
          <VAlert title="高危操作警告" type="warning" variant="tonal">
            此操作将使用备份文件中的数据
            <strong class="warning-text">覆盖</strong>
            数据库中对应的表。这是一个
            <strong class="warning-text">不可逆</strong>
            的过程！
            <br />
            <strong>请确认备份文件来源可靠</strong>
            ，跨环境导入可能导致数据错乱。
          </VAlert>
          <div>
            <span class="font-weight-medium">选择要恢复的表 (从文件中自动读取)</span>
            <div
              style="margin-left: 20px; display: inline-flex; vertical-align: middle"
              class="d-flex ga-3 flex-wrap align-center"
            >
              <VBtn color="primary" @click="selectAllForImport" variant="text" size="x-small">
                <template #prepend><UiIcon name="select" /></template>
                全选
              </VBtn>
              <VBtn color="primary" @click="deselectAllForImport" variant="text" size="x-small">全不选</VBtn>
            </div>
          </div>
          <div style="margin-top: 8px" class="settings-choice-group">
            <VRow class="settings-grid">
              <VCol cols="12" :md="12 / (isMobile ? 1 : 2)" v-for="table in tablesInBackupFile" :key="table">
                <VCheckbox
                  :value="table"
                  v-model="tablesToImport"
                  multiple
                  @update:model-value="handleImportSelectionChange"
                >
                  <template #label>{{ tableInfo[table]?.cn || table }}</template>
                </VCheckbox>
              </VCol>
            </VRow>
          </div>
        </div>
      </VCardText>
      <VCardActions class="justify-end flex-wrap ga-2">
        <VBtn @click="cancelImport" color="secondary">取消</VBtn>
        <VBtn color="primary" @click="confirmImport" :disabled="tablesToImport.length === 0">
          <template #prepend><UiIcon name="refresh" /></template>
          确认并开始恢复
        </VBtn>
      </VCardActions>
    </VCard>
  </VDialog>
  <!-- 清空指定表模态框 -->
  <VDialog
    v-model="clearTablesModalVisible"
    :fullscreen="isMobile"
    max-width="520"
    width="calc(100vw - 24px)"
    @update:model-value="$event || (clearTablesModalVisible = false)"
  >
    <VCard class="settings-dialog">
      <VCardTitle class="d-flex align-center justify-space-between ga-2">
        <span class="text-h6">清空指定数据表</span>
        <VBtn
          icon="mdi-close" color="default"
          variant="text"
          size="small"
          aria-label="关闭清空指定数据表"
          @click="clearTablesModalVisible = false"
        />
      </VCardTitle>
      <VCardText>
        <div style="margin-bottom: 10px" class="d-flex ga-3 flex-wrap align-center justify-end">
          <VBtn color="primary" @click="selectAllForClear" variant="text">
            <template #prepend><UiIcon name="select" /></template>
            全选
          </VBtn>
          <VBtn color="primary" @click="deselectAllForClear" variant="text">全不选</VBtn>
        </div>
        <VAlert title="高危操作警告" type="error" style="margin-bottom: 15px" variant="tonal">
          此操作将
          <strong class="warning-text">永久删除</strong>
          所选表中的所有数据，且
          <strong class="warning-text">不可恢复</strong>
          ！请务必谨慎操作。
        </VAlert>
        <div class="settings-choice-group">
          <VRow class="settings-grid">
            <VCol cols="12" :md="12 / (isMobile ? 1 : 2)" v-for="table in allDbTables" :key="table">
              <VCheckbox
                :value="table"
                v-model="tablesToClear"
                multiple
                @update:model-value="handleClearSelectionChange"
              >
                <template #label>{{ tableInfo[table]?.cn || table }}</template>
              </VCheckbox>
            </VCol>
          </VRow>
        </div>
      </VCardText>
      <VCardActions class="justify-end flex-wrap ga-2">
        <VBtn @click="clearTablesModalVisible = false" color="secondary">取消</VBtn>
        <VBtn color="error" @click="handleClearTables" :disabled="tablesToClear.length === 0" :loading="isClearing">
          <template #prepend><UiIcon name="delete" /></template>
          确认清空
        </VBtn>
      </VCardActions>
    </VCard>
  </VDialog>
  <!-- 重置演员映射模态框 -->
  <VDialog
    v-model="resetMappingsModalVisible"
    :fullscreen="isMobile"
    max-width="520"
    width="calc(100vw - 24px)"
    @update:model-value="$event || (resetMappingsModalVisible = false)"
  >
    <VCard class="settings-dialog">
      <VCardTitle class="d-flex align-center justify-space-between ga-2">
        <span class="text-h6">确认重置Emby数据</span>
        <VBtn
          icon="mdi-close" color="default"
          variant="text"
          size="small"
          aria-label="关闭确认重置Emby数据"
          @click="resetMappingsModalVisible = false"
        />
      </VCardTitle>
      <VCardText>
        <VAlert title="高危操作警告" type="warning" style="margin-bottom: 15px" variant="tonal">
          <p style="margin: 0 0 8px 0">
            此操作将
            <strong>清空所有Emby相关数据</strong>
            。
          </p>
          <p style="margin: 0 0 8px 0">它会保留宝贵的 元数据以及演员映射，以便在全量扫描后自动重新关联。</p>
          <p class="warning-text" style="margin: 0">
            <strong>请仅在您已经或将要重建 Emby 媒体库时执行此操作。</strong>
          </p>
        </VAlert>
      </VCardText>
      <VCardActions class="justify-end flex-wrap ga-2">
        <VBtn @click="resetMappingsModalVisible = false" color="secondary">取消</VBtn>
        <VBtn color="warning" @click="handleResetActorMappings" :loading="isResettingMappings">
          <template #prepend><UiIcon name="settings" /></template>
          确认重置
        </VBtn>
      </VCardActions>
    </VCard>
  </VDialog>
  <!-- AI 提示词配置模态框 -->
  <VDialog
    v-model="promptModalVisible"
    :fullscreen="isMobile"
    max-width="800"
    width="calc(100vw - 24px)"
    @update:model-value="$event || (promptModalVisible = false)"
  >
    <VCard class="settings-dialog">
      <VCardTitle class="d-flex align-center justify-space-between ga-2">
        <span class="text-h6">配置 AI 提示词</span>
        <VBtn
          icon="mdi-close" color="default"
          variant="text"
          size="small"
          aria-label="关闭配置 AI 提示词"
          @click="promptModalVisible = false"
        />
      </VCardTitle>
      <VCardText>
        <VAlert type="info" style="margin-bottom: 16px" variant="tonal">
          您可以自定义发送给 AI 的系统指令（System Prompt）。
          <br />
          <b>注意：</b>
          请保留关键的 JSON 输出格式要求，否则会导致解析失败。支持使用
          <code>{title}</code>
          等占位符。
        </VAlert>
        <div class="settings-loading">
          <VProgressLinear v-if="loadingPrompts" indeterminate color="primary" class="mb-3" />
          <VTabs v-model="activePromptTab" class="mb-4" show-arrows>
            <VTab value="fast_mode">
              <span class="mp-title-with-icon">
                <UiIcon name="settings" :size="16" />
                快速模式 (人名)
              </span>
            </VTab>
            <VTab value="quality_mode">
              <span class="mp-title-with-icon">
                <UiIcon name="settings" :size="16" />
                顾问模式 (人名)
              </span>
            </VTab>
            <VTab value="overview_translation">
              <span class="mp-title-with-icon">
                <UiIcon name="settings" :size="16" />
                简介翻译
              </span>
            </VTab>
            <VTab value="title_translation">
              <span class="mp-title-with-icon">
                <UiIcon name="settings" :size="16" />
                标题翻译
              </span>
            </VTab>
            <VTab value="transliterate_mode">
              <span class="mp-title-with-icon">
                <UiIcon name="settings" :size="16" />
                音译模式
              </span>
            </VTab>
            <VTab value="batch_joke_fallback">
              <span class="mp-title-with-icon">
                <UiIcon name="settings" :size="16" />
                占位简介
              </span>
            </VTab>
          </VTabs>
          <VWindow v-model="activePromptTab" :touch="false">
            <VWindowItem value="fast_mode" eager>
              <VTextarea
                v-model="promptsModel.fast_mode"
                placeholder="输入提示词..."
                style="font-family: monospace"
                auto-grow
                rows="10"
                max-rows="20"
              ></VTextarea>
            </VWindowItem>
            <VWindowItem value="quality_mode" eager>
              <VTextarea
                v-model="promptsModel.quality_mode"
                placeholder="输入提示词..."
                style="font-family: monospace"
                auto-grow
                rows="10"
                max-rows="20"
              ></VTextarea>
            </VWindowItem>
            <VWindowItem value="overview_translation" eager>
              <VTextarea
                v-model="promptsModel.overview_translation"
                placeholder="输入提示词..."
                style="font-family: monospace"
                auto-grow
                rows="10"
                max-rows="20"
              ></VTextarea>
              <span style="font-size: 12px" class="text-medium-emphasis">可用变量: {title}, {overview}</span>
            </VWindowItem>
            <VWindowItem value="title_translation" eager>
              <VTextarea
                v-model="promptsModel.title_translation"
                placeholder="输入提示词..."
                style="font-family: monospace"
                auto-grow
                rows="10"
                max-rows="20"
              ></VTextarea>
              <span style="font-size: 12px" class="text-medium-emphasis">可用变量: {media_type}, {title}, {year}</span>
            </VWindowItem>
            <VWindowItem value="transliterate_mode" eager>
              <VTextarea
                v-model="promptsModel.transliterate_mode"
                placeholder="输入提示词..."
                style="font-family: monospace"
                auto-grow
                rows="10"
                max-rows="20"
              ></VTextarea>
            </VWindowItem>
            <VWindowItem value="batch_joke_fallback" eager>
              <VTextarea
                v-model="promptsModel.batch_joke_fallback"
                placeholder="输入提示词..."
                style="font-family: monospace"
                auto-grow
                rows="10"
                max-rows="20"
              ></VTextarea>
            </VWindowItem>
          </VWindow>
        </div>
      </VCardText>
      <VCardActions class="justify-end flex-wrap ga-2">
        <div style="width: 100%" class="d-flex ga-3 flex-wrap align-center justify-space-between">
          <VBtn
            color="warning"
            :loading="savingPrompts"
            @click="confirmSettingAction('确定要丢弃所有自定义修改，恢复到系统默认提示词吗？', resetPrompts, '确认')"
            variant="outlined"
          >
            <template #prepend><UiIcon name="refresh" /></template>
            恢复默认
          </VBtn>
          <div class="d-flex ga-3 flex-wrap align-center">
            <VBtn @click="promptModalVisible = false" color="secondary">取消</VBtn>
            <VBtn color="primary" @click="savePrompts" :loading="savingPrompts">
              <template #prepend><UiIcon name="save" /></template>
              保存配置
            </VBtn>
          </div>
        </div>
      </VCardActions>
    </VCard>
  </VDialog>
</template>

<script setup>
import { ref, reactive, watch, computed, onMounted, onUnmounted, nextTick } from 'vue';
import { useMessage, useDialog } from '@/composables/useFeedback';
import { useDisplay } from 'vuetify';
import SettingsQrCode from './general/SettingsQrCode.vue';
import SettingCard from './general/SettingCard.vue';
import LocalPathField from './general/LocalPathField.vue';
import SettingsSearchResults from './general/SettingsSearchResults.vue';
import { useSettingsSearch } from '@/composables/useSettingsSearch';
import { useRoute } from 'vue-router';
const ExportIcon = 'mdi-download';
const ImportIcon = 'mdi-cloud-upload';
const ClearIcon = 'mdi-delete-outline';
const BuildIcon = 'mdi-wrench';
const SyncIcon = 'mdi-sync';
const FolderIcon = 'mdi-folder-outline';
const CheckIcon = 'mdi-check-circle-outline';
const CloseIcon = 'mdi-close-circle-outline';
const ArrowUpIcon = 'mdi-arrow-up';
const RefreshIcon = 'mdi-refresh';
const FileIcon = 'mdi-file-document-outline';
const SaveIcon = 'mdi-content-save-outline';
const SelectAllIcon = 'mdi-select-all';
import { useConfig } from '../../composables/useConfig.js';
import axios from 'axios';

const route = useRoute();
const activeSettingsTab = computed(() => ['system', 'directory', 'rule', 'search'].includes(route.query.tab) ? route.query.tab : 'system');
const activePromptTab = ref('fast_mode');
const revealedPasswords = reactive({});
const importFiles = ref([]);
const isPreviewingImport = ref(false);

const { smAndDown: isMobile } = useDisplay();

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
const { configModel, loadingConfig, savingConfig, configError, handleSaveConfig } = useConfig();
const settingsSearch = useSettingsSearch('general', { ready: computed(() => Boolean(configModel.value) && !loadingConfig.value), disabled: loadingConfig });
const isPreviewingRenameFormats = ref(false);
const monitorExtensionOptions = ['.mp4', '.mkv', '.ts', '.iso', '.rmvb', '.avi', '.mov', '.mpeg', '.mpg', '.wmv', '.3gp', '.asf', '.m4v', '.flv', '.m2ts', '.tp', '.f4v', '.strm'];
const renameFormatPreview = ref({ movie: '', tv: '', errors: {} });
let renamePreviewRequest = 0;
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
const embyUserIdValidation = [value => !value || embyUserIdRegex.test(value) || 'ID格式不正确，应为32位。'];
const confirmSettingAction = (content, action, positiveText = '确认') => {
  dialog.warning({ title: '确认操作', content, positiveText, negativeText: '取消', onPositiveClick: action });
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
const p115StatusError = ref('');

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
  stopCookiePolling();
  const requestVersion = cookieQrcodeRequestVersion;
  const isCurrentSession = () => componentIsMounted.value && showCookieModal.value
    && requestVersion === cookieQrcodeRequestVersion
    && sessionId === cookieQrcodeSessionId.value && boundClientType === cookieAppType.value;
  let pending = false;
  cookieQrcodePolling.value = setInterval(async () => {
    if (!isCurrentSession() || cookieQrcodeStatus.value !== 'waiting' || pending) return;
    pending = true;
    try {
      const res = await axios.get('/api/p115/cookie_qrcode/status', {
        params: { session_id: sessionId }
      });
      if (!isCurrentSession() || cookieQrcodeStatus.value !== 'waiting') return;
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
          if (!isCurrentSession() || cookieQrcodeStatus.value !== 'success') return;
          closeCookieModal();
          check115Status();
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
      if (!isCurrentSession() || cookieQrcodeStatus.value !== 'waiting') return;
      const data = e.response?.data;
      if (data?.status === 'expired') {
        cookieQrcodeStatus.value = 'expired';
      } else {
        cookieQrcodeStatus.value = 'error';
        message.error(data?.message || '检查 Cookie 二维码状态失败');
      }
      stopCookiePolling();
    } finally {
      pending = false;
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
let localFolderRequestVersion = 0;
const loadedLocalFolder = ref(null);
const localFolderSelectionReady = computed(() => !!loadedLocalFolder.value
  && loadedLocalFolder.value.field === currentLocalTargetField.value
  && loadedLocalFolder.value.isArray === isCurrentLocalTargetArray.value
  && loadedLocalFolder.value.path === currentLocalPath.value);

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
    const requestVersion = ++localFolderRequestVersion;
    const requestedPath = String(path || '/');
    const targetField = currentLocalTargetField.value;
    const targetIsArray = isCurrentLocalTargetArray.value;
    const isCurrentRequest = () => requestVersion === localFolderRequestVersion
        && componentIsMounted.value && showLocalFolderModal.value
        && targetField === currentLocalTargetField.value
        && targetIsArray === isCurrentLocalTargetArray.value
        && currentLocalPath.value === requestedPath;
    currentLocalPath.value = requestedPath;
    loadedLocalFolder.value = null;
    localFolders.value = [];
    loadingLocalFolders.value = true;
    try {
        const res = await axios.get('/api/p115/system/directories', { params: { path: requestedPath } });
        if (!isCurrentRequest()) return;
        if (res.data.code === 200 && typeof res.data.current_path === 'string' && Array.isArray(res.data.data)) {
            localFolders.value = res.data.data;
            currentLocalPath.value = res.data.current_path;
            loadedLocalFolder.value = { field: targetField, isArray: targetIsArray, path: res.data.current_path };
        } else {
            message.error(res.data.message || '获取目录失败，尚未确认当前路径');
        }
    } catch (error) {
        if (!isCurrentRequest()) return;
        if (error.response && error.response.status === 403) {
            message.error('没有权限访问该目录！');
        } else if (error.response && error.response.status === 404) {
            message.error('目录不存在！');
        } else {
            message.error('请求目录失败: ' + (error.response?.data?.message || error.message));
        }
    } finally {
        if (requestVersion === localFolderRequestVersion) loadingLocalFolders.value = false;
    }
};

// 点击列表中的文件夹 (保持不变)
const selectLocalFolder = (folder) => {
    fetchLocalFolders(folder.path)
}

// 确认选择 (增强版)
const confirmLocalFolder = () => {
    if (!componentIsMounted.value || !showLocalFolderModal.value) return;
    if (loadingLocalFolders.value || !localFolderSelectionReady.value) {
        message.warning('请先加载并确认当前目录，再保存路径。');
        return;
    }
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
let p115FolderRequestVersion = 0;
const loadedP115Folder = ref(null);
const p115FolderSelectionReady = computed(() => !!loadedP115Folder.value
  && loadedP115Folder.value.cid === currentP115Cid.value
  && loadedP115Folder.value.path === currentP115Path.value);

const openP115FolderSelector = () => {
    currentP115Cid.value = configModel.value.p115_fingerprint_media_root_cid || '0'
    currentP115Path.value = configModel.value.p115_fingerprint_media_root_path || ''
    showP115FolderModal.value = true
    fetchP115Folders(currentP115Cid.value)
}

const fetchP115Folders = async (cid = '0') => {
    const requestVersion = ++p115FolderRequestVersion;
    const requestedCid = String(cid || '0');
    const isCurrentRequest = () => requestVersion === p115FolderRequestVersion
        && componentIsMounted.value && showP115FolderModal.value
        && currentP115Cid.value === requestedCid;
    currentP115Cid.value = requestedCid;
    currentP115Path.value = '';
    loadedP115Folder.value = null;
    p115Folders.value = [];
    loadingP115Folders.value = true;
    try {
        const res = await axios.get('/api/p115/directories', { params: { cid: requestedCid } });
        if (!isCurrentRequest()) return;
        if (res.data.code === 200 && res.data.current_cid != null
            && typeof res.data.current_path === 'string' && Array.isArray(res.data.data)) {
            p115Folders.value = res.data.data;
            currentP115Cid.value = String(res.data.current_cid);
            currentP115Path.value = res.data.current_path;
            loadedP115Folder.value = { cid: currentP115Cid.value, path: currentP115Path.value };
        } else {
            message.error(res.data.message || '获取 115 目录失败，尚未确认当前路径');
        }
    } catch (error) {
        if (!isCurrentRequest()) return;
        message.error('请求 115 目录失败: ' + (error.response?.data?.message || error.message));
    } finally {
        if (requestVersion === p115FolderRequestVersion) loadingP115Folders.value = false;
    }
};

const selectP115Folder = (folder) => {
    fetchP115Folders(folder.cid)
}

const confirmP115Folder = () => {
    if (!componentIsMounted.value || !showP115FolderModal.value) return;
    if (loadingP115Folders.value || !p115FolderSelectionReady.value) {
        message.warning('请先加载并确认当前 115 目录，再保存路径。');
        return;
    }
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
let openApiQrcodeRequestVersion = 0;
const handleOpenQrcodeModal = () => {
  if (!configModel.value.p115_app_id || !configModel.value.p115_app_id.trim()) {
    message.warning('请先填写自定义 AppID 并点击底部保存设置，然后再扫码授权！');
    return;
  }
  openQrcodeModal();
};
const openQrcodeModal = async () => {
  stopPolling();
  const requestVersion = ++openApiQrcodeRequestVersion;
  showQrcodeModal.value = true;
  qrcodeStatus.value = 'loading';
  qrcodeLoading.value = true;

  try {
    const res = await axios.post('/api/p115/qrcode');
    if (requestVersion !== openApiQrcodeRequestVersion || !showQrcodeModal.value) return;
    if (res.data && res.data.success) {
      qrcodeUrl.value = res.data.data.qrcode;
      qrcodeStatus.value = 'waiting';
      startPolling();
    } else {
      qrcodeStatus.value = 'error';
      message.error(res.data?.message || '获取二维码失败');
    }
  } catch (e) {
    if (requestVersion !== openApiQrcodeRequestVersion || !showQrcodeModal.value) return;
    qrcodeStatus.value = 'error';
    message.error('获取二维码失败: ' + (e.response?.data?.message || e.message));
  } finally {
    if (requestVersion === openApiQrcodeRequestVersion) qrcodeLoading.value = false;
  }
};

const startPolling = () => {
  stopPolling();
  const requestVersion = openApiQrcodeRequestVersion;
  // 每2秒检查一次二维码状态
  qrcodePolling.value = setInterval(async () => {
    if (requestVersion !== openApiQrcodeRequestVersion || !showQrcodeModal.value) return;
    try {
      const res = await axios.get('/api/p115/qrcode/status');
      if (requestVersion !== openApiQrcodeRequestVersion || !showQrcodeModal.value) return;
      const data = res.data;

      if (data.status === 'success') {
        qrcodeStatus.value = 'success';
        message.success('登录成功！授权已自动保存');
        stopPolling();
        setTimeout(() => {
          if (requestVersion !== openApiQrcodeRequestVersion || !showQrcodeModal.value) return;
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
      if (requestVersion !== openApiQrcodeRequestVersion || !showQrcodeModal.value) return;
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
  openApiQrcodeRequestVersion += 1;
  stopPolling();
  showQrcodeModal.value = false;
  qrcodeUrl.value = '';
  qrcodeStatus.value = 'idle';
};

// ★★★ 全自动网页授权 (授权码模式) 逻辑 ★★★
const isWebAuthing = ref(false);
let webAuthPolling = null;
let webAuthTimeout = null;
let webAuthRequestVersion = 0;

const stopWebAuth = () => {
  if (webAuthPolling) clearInterval(webAuthPolling);
  if (webAuthTimeout) clearTimeout(webAuthTimeout);
  webAuthPolling = null;
  webAuthTimeout = null;
  isWebAuthing.value = false;
};

const startWebAuth = () => {
  stopWebAuth();
  const requestVersion = ++webAuthRequestVersion;
  const callbackUrl = `${window.location.origin}/api/p115/auto_save_auth`;
  const authUrl = `https://115.55565576.xyz/login?callback_url=${encodeURIComponent(callbackUrl)}`;
  const width = 500;
  const height = 600;
  const left = (window.screen.width / 2) - (width / 2);
  const top = (window.screen.height / 2) - (height / 2);
  let authWindow = null;
  try {
    authWindow = window.open(authUrl, '_blank', `width=${width},height=${height},top=${top},left=${left}`);
  } catch {
    // Sandboxed or blocked popups follow the same visible failure path.
  }
  if (!authWindow) {
    message.warning('浏览器阻止了授权窗口，请允许弹出窗口后重试。');
    return;
  }

  isWebAuthing.value = true;
  message.info('请在新弹出的窗口中完成 115 授权...', { duration: 5000 });
  let callbackConfirmed = false;
  let checkingStatus = false;
  webAuthPolling = setInterval(async () => {
    if (requestVersion !== webAuthRequestVersion || !componentIsMounted.value || !isWebAuthing.value) return;
    try {
      // This existing callback returns the success page only after saving Token.
      callbackConfirmed ||= authWindow.location.origin === window.location.origin
        && authWindow.location.pathname === '/api/p115/auto_save_auth'
        && authWindow.document.title === '授权成功';
    } catch {
      // The authorization provider is cross-origin until it redirects back.
    }
    if (authWindow.closed && !callbackConfirmed) {
      stopWebAuth();
      message.warning('授权窗口已关闭，尚未确认本次授权；原凭证保持不变。');
      return;
    }
    if (!callbackConfirmed || checkingStatus) return;
    checkingStatus = true;
    try {
      const res = await axios.get('/api/p115/status');
      if (requestVersion !== webAuthRequestVersion || !componentIsMounted.value || !isWebAuthing.value) return;
      if (res.data?.data?.has_token === true) {
        p115Info.value = res.data.data;
        stopWebAuth();
        message.success('网页授权成功！Token 已自动保存。');
      }
    } catch {
      // Keep existing credentials and retry only this confirmed callback.
    } finally {
      checkingStatus = false;
    }
  }, 2000);

  webAuthTimeout = setTimeout(() => {
    if (requestVersion === webAuthRequestVersion && componentIsMounted.value && isWebAuthing.value) {
      stopWebAuth();
      message.warning('授权等待超时，尚未确认本次授权；原凭证保持不变。');
    }
  }, 180000);
};

// 检查 115 状态
const check115Status = async () => {
    loading115Info.value = true;
    p115StatusError.value = '';
    try {
        const res = await axios.get('/api/p115/status');
        if (res.data && res.data.data) {
            p115Info.value = res.data.data;
            message.success('115 状态刷新成功！');
        } else {
            p115Info.value = null;
            p115StatusError.value = '状态响应无效';
        }
    } catch (e) {
        p115Info.value = null;
        p115StatusError.value = '状态读取失败';
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
  basic: ['delay_between_items_sec', 'media_info_retry_interval', 'media_info_retry_attempts', 'subtitle_chinese_default_language', 'max_actors_to_process', 'max_episode_actors_to_process', 'extract_thumb', 'actor_role_add_prefix', 'remove_actors_without_avatars', 'keyword_to_tags', 'studio_to_chinese'],
  monitor: ['monitor_enabled', 'monitor_paths', 'monitor_exclude_dirs', 'monitor_exclude_refresh_delay', 'monitor_extensions', 'tmdb_image_language_preference', 'monitor_sha1_pc_search'],
  sources: ['tmdb_api_key', 'tmdb_api_base_url', 'douban_enable_online_api', 'api_douban_default_cooldown_seconds', 'douban_cookie'],
  p115Account: ['p115_auth_method', 'p115_app_id', 'p115_api_priority', 'p115_request_interval'],
  fingerprints: ['emby_metadata_path', 'local_strm_root', 'cd2_media_root', 'p115_fingerprint_media_root_cid', 'p115_fingerprint_media_root_path', 'p115_generate_mediainfo', 'p115_enable_sync_delete'],
  emby: ['emby_server_url', 'emby_public_url', 'emby_api_key', 'emby_webhook_secret', 'emby_webhook_ingest_enabled', 'emby_user_id', 'emby_admin_user', 'emby_admin_pass', 'emby_api_timeout', 'libraries_to_ignore'],
  ai: ['ai_provider', 'ai_api_key', 'ai_model_name', 'ai_base_url', 'ai_request_timeout', 'ai_max_retries', 'ai_translation_batch_size', 'ai_translate_actor_role', 'ai_translate_on_sync', 'ai_translate_title', 'ai_translate_overview', 'ai_translate_episode_overview', 'ai_joke_fallback', 'ai_translation_mode'],
  logs: ['log_rotation_size_mb', 'log_rotation_backup_count'],
  ingest: ['auto_repair_truehd_core_on_ingest', 'auto_fill_language_titles_on_ingest', 'auto_extract_intro_fingerprint_on_ingest', 'auto_extract_video_thumbnails_on_ingest', 'extract_video_thumbnails_after_save'],
  rename: ['movie_rename_format', 'tv_rename_format'],
};
const savingCard = ref(null);
const saveCard = async (cardKey) => {
  const fields = cardFields[cardKey];
  if (!fields || savingConfig.value || !configModel.value) return;
  try {
    if (fields.includes('emby_user_id')) {
      const validation = await formRef.value?.validate();
      if (validation?.valid === false) throw new Error('Emby 用户 ID 格式不正确');
    }
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
  const request = ++renamePreviewRequest;
  isPreviewingRenameFormats.value = true;
  try {
    const response = await axios.post('/api/config/rename_preview', {
      movie_format: configModel.value.movie_rename_format,
      tv_format: configModel.value.tv_rename_format,
    });
    if (request !== renamePreviewRequest || !componentIsMounted.value) return;
    renameFormatPreview.value = { ...(response.data || {}), errors: response.data?.errors || {} };
  } catch (error) {
    if (request !== renamePreviewRequest || !componentIsMounted.value) return;
    const reason = error.response?.data?.error || '读取近期入库或预览重命名格式失败';
    renameFormatPreview.value = { movie: '', tv: '', errors: { movie: reason, tv: reason } };
  } finally {
    if (request === renamePreviewRequest) isPreviewingRenameFormats.value = false;
  }
};
watch(
  () => [configModel.value?.movie_rename_format, configModel.value?.tv_rename_format],
  () => {
    if (!configModel.value) return;
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

const handleImportFileSelection = async (selection) => {
  const rawFile = Array.isArray(selection) ? selection[0] : selection;
  importFiles.value = rawFile ? [rawFile] : [];
  if (!rawFile || isPreviewingImport.value || isImporting.value) return;
  isPreviewingImport.value = true;
  try {
    await handleCustomImportRequest({
      file: { file: rawFile },
      onFinish: () => {},
      onError: () => { importFiles.value = []; },
    });
  } finally {
    isPreviewingImport.value = false;
  }
};

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
  componentIsMounted.value = false;
  renamePreviewRequest += 1;
  if (renamePreviewTimer) clearTimeout(renamePreviewTimer);
  cookieQrcodeRequestVersion += 1;
  openApiQrcodeRequestVersion += 1;
  webAuthRequestVersion += 1;
  localFolderRequestVersion += 1;
  p115FolderRequestVersion += 1;
  stopCookiePolling();
  stopPolling();
  stopWebAuth();
  if (unwatchGlobal) unwatchGlobal();
  if (unwatchEmbyConfig) unwatchEmbyConfig();
});
</script>

<style scoped>
.settings-container { min-width: 0; }
.settings-header { margin-block-end: 24px; }
.settings-grid { align-items: stretch; }
.settings-grid > .v-col { min-width: 0; }
.settings-card-body, .ai-settings-wrapper {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 20px 24px;
  align-items: start;
}
.settings-card-body > *, .ai-settings-wrapper > * { grid-column: 1 / -1; min-width: 0; }
.settings-card-body > .settings-field, .ai-settings-wrapper > .settings-field { grid-column: auto; }
.settings-field { min-width: 0; }
.settings-hint, .description-text {
  color: rgba(var(--v-theme-on-surface), 0.68);
  font-size: 12px;
  line-height: 1.7;
  margin-block-start: 6px;
}
.settings-input-group { min-width: 0; }
.settings-input-group > :deep(.v-input) { flex: 1; min-width: 0; }
.settings-input-group > :deep(.v-btn) { flex-shrink: 0; margin-block-start: 8px; }
.settings-choice-group :deep(.v-selection-control) { min-width: 0; }
.settings-loading { min-width: 0; }
.settings-dialog :deep(.v-card-title) { white-space: normal; }
.settings-dialog :deep(.v-card-text) { min-width: 0; }
.settings-dialog :deep(.v-btn) { white-space: normal; }
.settings-divider { grid-column: 1 / -1; margin-block: 4px; }
.card-title { display: inline-flex; align-items: center; gap: 8px; }
.content-disabled { opacity: 0.6; }
.ai-settings-wrapper { transition: opacity 0.3s ease; }
.engine-list { display: flex; flex-wrap: wrap; gap: 8px; }
.engine-tag { cursor: grab; }
.engine-tag:active { cursor: grabbing; }
.drag-handle { margin-inline-end: 6px; vertical-align: -0.15em; }
.warning-text { color: rgb(var(--v-theme-warning)); font-weight: 600; }
.data-action-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(min(100%, 180px), 1fr));
  gap: 12px;
  align-items: start;
}
.data-action-grid .action-button { width: 100%; min-width: 0; min-height: 48px; }
.backup-file-input { min-width: 0; }
.info-icon { color: rgb(var(--v-theme-info)); cursor: help; }
.rename-format-grid { display: grid; grid-template-columns: minmax(0, 1.15fr) minmax(0, 0.85fr); gap: 20px; }
.rename-format-form { min-width: 0; }
.rename-preview-panel { min-width: 0; padding: 16px; border: thin solid rgba(var(--v-border-color), var(--v-border-opacity)); border-radius: var(--v-border-radius, 10px); }
.rename-preview-title { margin-block-end: 8px; font-size: 13px; font-weight: 500; }
.rename-preview-tree { min-height: 28px; }
.rename-preview-node { display: flex; align-items: center; gap: 7px; min-height: 26px; font-family: monospace; font-size: 12px; overflow-wrap: anywhere; }
@media (max-width: 1000px) {
  .settings-card-body, .ai-settings-wrapper, .rename-format-grid { grid-template-columns: minmax(0, 1fr); }
}
@media (max-width: 600px) {
  .settings-header { margin-block-end: 16px; }
  .settings-card-body, .ai-settings-wrapper { gap: 16px; }
  .settings-page :deep(.v-card-item) { align-items: start; flex-wrap: wrap; gap: 8px; }
  .settings-page :deep(.v-card-item__append) { padding-inline-start: 0; }
  .settings-input-group { flex-wrap: wrap; }
  .settings-input-group > :deep(.v-input) { flex-basis: 100%; }
  .settings-input-group > :deep(.v-btn) { margin-block-start: 0; }
  .data-action-grid { grid-template-columns: 1fr; }
}
</style>
