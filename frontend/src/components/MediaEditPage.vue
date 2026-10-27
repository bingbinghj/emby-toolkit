<!-- src/components/MediaEditPage.vue -->
<template>
  <n-layout class="mp-page editor-page" content-style="padding: 0;">
    <div class="media-edit-page">
      <n-page-header class="mp-page-header" @back="goBack">
        <template #title>
          手动编辑媒体信息
        </template>
      </n-page-header>

      <n-divider />

      <div v-if="isLoading" class="loading-container">
        <n-spin size="large" />
        <p style="text-align: center; margin-top: 10px;">正在加载媒体详情...</p>
      </div>

      <div v-else-if="itemDetails && itemDetails.item_name">
        <n-grid cols="1 920:4" :x-gap="24" :y-gap="24">
          <!-- 左侧信息栏 (海报) -->
          <n-grid-item span="1">
            <n-space vertical :size="24">
              <n-card :bordered="false"><template #header>{{ itemDetails.item_name }}</template>
                <template #cover>
                  <n-image
                    :src="posterUrl"
                    lazy
                    object-fit="cover"
                    class="media-poster"
                    :fallback-src="fallbackAvatar"
                  >
                    <template #placeholder>
                      <div class="poster-placeholder">
                        <n-icon :component="ImageIcon" size="48" :depth="3" />
                      </div>
                    </template>
                  </n-image>
                </template>
                <template #header-extra>
                  <n-tag :type="itemDetails.item_type === 'Movie' ? 'info' : 'success'" size="small" round>
                    {{ itemTypeInChinese }}
                  </n-tag>
                </template>
                <n-descriptions label-placement="left" bordered :column="1" size="small">
                  <n-descriptions-item label="Emby ItemID">
                    {{ itemDetails.item_id }}
                  </n-descriptions-item>
                  <n-descriptions-item label="原始记录评分" v-if="itemDetails.original_score !== null && itemDetails.original_score !== undefined">
                    <n-tag type="warning" size="small">{{ itemDetails.original_score }}</n-tag>
                  </n-descriptions-item>
                  <n-descriptions-item label="待复核原因" v-if="itemDetails.review_reason">
                    <n-text type="error">{{ itemDetails.review_reason }}</n-text>
                  </n-descriptions-item>
                </n-descriptions>

                <!-- ★★★ 编辑图像按钮 ★★★ -->
                <template #action>
                  <n-space vertical>
                    <n-button block type="primary" secondary @click="showImageEditor = true">
                      <template #icon>
                        <n-icon :component="ImagesIcon" />
                      </template>
                      编辑图像
                    </n-button>
                    <!-- ▼▼▼ 编辑媒体信息按钮 ▼▼▼ -->
                    <n-button
                      block
                      type="info"
                      secondary
                      @click="handleMediaInfoButtonClick"
                    >
                      <template #icon>
                        <n-icon :component="DocumentTextIcon" />
                      </template>
                      编辑媒体信息 (音轨/字幕)
                    </n-button>
                    <n-button block secondary :loading="isRevalidatingFingerprints"
                      @click="revalidateFingerprints(itemDetails.item_id || itemId)"><template #icon><UiIcon name="check" /></template>
                      重新校验指纹（{{ fingerprintScopeLabel }}）
                    </n-button>
                    <n-popconfirm
                      v-if="['Series', 'Season', 'Episode'].includes(itemDetails.item_type)"
                      @positive-click="replaceEpisodeImages"
                    >
                      <template #trigger>
                        <n-button
                          block
                          type="warning"
                          secondary
                          :loading="isReplacingEpisodeImages"
                        >
                          <template #icon><n-icon :component="ImagesIcon" /></template>
                          替换剧集图
                        </n-button>
                      </template>
                      将删除当前{{ itemDetails.item_type === 'Episode' ? '单集' : itemDetails.item_type === 'Season' ? '季所有分集' : '电视剧所有分集' }}的 Primary 海报，随后让神医助手使用 ffprobe 截图生成集图。是否继续？
                    </n-popconfirm>
                    <n-button
                      block
                      type="warning"
                      secondary
                      @click="openRenameEditor"
                    >
                      <template #icon>
                        <n-icon :component="RepairIcon" />
                      </template>
                      重命名媒体
                    </n-button>
                    <n-popconfirm @positive-click="repairCurrentP115Orphans">
                      <template #trigger>
                        <n-button
                          block
                          type="warning"
                          secondary
                          :loading="isRepairingP115Orphan"
                        >
                          <template #icon>
                            <n-icon :component="CloudIcon" />
                          </template>
                          检查115孤儿文件
                        </n-button>
                      </template>
                      确认只检查{{ p115OrphanScopeLabel }}？
                    </n-popconfirm>
                  </n-space>
                </template>
              </n-card>

              <n-card :bordered="false"><template #header>辅助工具</template>
                <n-space vertical>
                  <n-form-item label="数据操作" label-placement="top">
                    <n-space>
                      <n-button-group>
                      <n-button
                        tag="a"
                        :href="searchLinks.baidu"
                        target="_blank"
                        :disabled="!searchLinks.baidu"
                        :loading="isLoading"
                      ><template #icon><UiIcon name="search" /></template>
                        百度搜索
                      </n-button>
                      <n-dropdown
                        trigger="click"
                        :options="searchDropdownOptions"
                        @select="handleSearchDropdownSelect"
                      >
                        <n-button :disabled="searchDropdownOptions.length === 0">
                          <template #icon>
                            <n-icon :component="ChevronDownIcon" />
                          </template>
                        </n-button>
                      </n-dropdown>
                    </n-button-group>
                      <n-button
                        type="info"
                        @click="translateAllFields"
                        :loading="isTranslating"
                        :disabled="isLoading"
                      ><template #icon><UiIcon name="language" /></template>
                        一键翻译
                      </n-button>
                    </n-space>
                  </n-form-item>
                </n-space>
              </n-card>
            </n-space>
          </n-grid-item>

          <!-- 右侧演员列表 -->
          <n-grid-item span="1 920:3">
            <n-card :bordered="false" class="dashboard-card">
              <template #header>
                <span class="card-title">演员列表</span>
              </template>
              <n-form label-placement="top" label-width="auto">
                <draggable
                  v-model="editableCast"
                  tag="div"
                  item-key="_temp_id"
                  class="actor-grid-container"
                  handle=".drag-handle"
                  animation="300"
                >
                  <template #item="{ element: actor, index }">
                    <div class="actor-grid-item">
                      <n-card size="small" class="dashboard-card actor-edit-card" content-style="padding: 16px;">
                        <template #header>
                          <div class="actor-card-header">
                            <n-avatar
                              round
                              size="small"
                              :style="{ backgroundColor: getAvatarColor(actor.name), color: textColorForBackground(getAvatarColor(actor.name)) }"
                            >
                              {{ index + 1 }}
                            </n-avatar>
                            <span class="actor-name-title" :title="actor.name">{{ actor.name || '新演员' }}</span>
                          </div>
                        </template>
                        <template #header-extra>
                          <n-space>
                            <n-button text class="drag-handle">
                              <n-icon :component="DragHandleIcon" />
                            </n-button>
                            <n-popconfirm @positive-click="removeActor(index)">
                              <template #trigger>
                                <n-button text type="error">
                                  <n-icon :component="TrashIcon" />
                                </n-button>
                              </template>
                              确定要删除演员 “{{ actor.name || '新演员' }}” 吗？
                            </n-popconfirm>
                          </n-space>
                        </template>

                        <div class="actor-card-content">
                          <n-image
                            :src="getActorImageUrl(actor)"
                            lazy
                            object-fit="cover"
                            class="actor-avatar-image"
                          >
                            <template #placeholder>
                              <div class="avatar-placeholder">
                                <n-icon :component="PersonIcon" size="24" :depth="3" />
                              </div>
                            </template>
                          </n-image>

                          <div class="actor-inputs">
                            <n-form-item label="演员" label-placement="left" label-width="40" class="compact-form-item">
                              <n-input v-model:value="actor.name" placeholder="演员名" size="small" style="width: 100%;" />
                            </n-form-item>
                            <n-form-item label="角色" label-placement="left" label-width="40" class="compact-form-item">
                              <n-input v-model:value="actor.role" placeholder="角色名" size="small" style="width: 100%;" />
                            </n-form-item>
                          </div>
                        </div>
                      </n-card>
                    </div>
                  </template>
                </draggable>
              </n-form>

              <div class="sticky-actions">
                <n-space>
                  <n-button @click="showAddActorModal = true" type="default" secondary>
                    <template #icon>
                      <n-icon :component="AddIcon" />
                    </template>
                    添加演员
                  </n-button>
                  <n-button @click="goBack"><template #icon><UiIcon name="back" /></template>返回列表</n-button>
                  <n-button type="primary" @click="handleSaveChanges" :loading="isSaving"><template #icon><UiIcon name="save" /></template>
                    保存修改
                  </n-button>
                </n-space>
              </div>
            </n-card>
          </n-grid-item>
        </n-grid>
      </div>

      <div v-else class="error-container">
        <n-alert title="错误" type="error">
          无法加载媒体详情，或指定的媒体项不存在。请检查后端日志或确认该媒体项有效。
          <n-button text @click="goBack" style="margin-left: 10px;"><template #icon><UiIcon name="back" /></template>返回列表</n-button>
        </n-alert>
      </div>
    </div>

    <!-- ★★★ 剧集分集选择模态框 ★★★ -->
    <n-modal
      v-model:show="showEpisodeSelector"
      preset="card"
      style="width: 980px; max-width: 95vw;"
      :bordered="false"
      size="huge"
    ><template #header>{{ itemDetails?.item_type === 'Series' ? '选择季和要编辑的视频' : '选择要编辑的视频' }}</template>
      <n-spin :show="isFetchingEpisodes">
        <n-button secondary :loading="isRevalidatingFingerprints" style="margin-bottom: 12px;"
          @click="revalidateFingerprints(itemDetails.item_id || itemId)"><template #icon><UiIcon name="check" /></template>
          重新校验指纹（{{ fingerprintScopeLabel }}）
        </n-button>
        <div v-if="episodesList.length > 0" class="episode-selector-board">
          <div class="episode-selector-toolbar">
            <div v-if="itemDetails.item_type === 'Series'" class="episode-season-switcher">
              <span class="episode-season-switcher-label">选择季</span>
              <n-button
                v-for="season in episodeSeasonGroups"
                :key="`season-switch-${season.season_number}`"
                size="small"
                :type="season.season_number === selectedSeasonNumber ? 'primary' : 'default'"
                :secondary="season.season_number !== selectedSeasonNumber"
                @click="handleSeasonChange(season.season_number)"
              ><template #icon><UiIcon name="media" /></template>
                {{ formatSeasonLabel(season.season_number) }}
              </n-button>
            </div>
            <div class="episode-selector-summary">
              {{ itemDetails.item_type === 'Series' ? '当前季' : '本季' }} {{ visibleSeasonEpisodes.length }} 个视频，已选 {{ selectedEpisodeIds.length }} 个。
            </div>
            <n-space size="small">
              <n-select
                v-if="currentSeasonHasMultipleVersions"
                v-model:value="selectedBatchVersionIndex"
                :options="batchVersionOptions"
                size="small"
                style="width: 260px; max-width: 100%;"
                placeholder="选择版本"
              />
              <n-button
                v-if="currentSeasonHasMultipleVersions"
                size="small"
                secondary
                :disabled="selectedBatchVersionIndex === null"
                @click="selectEpisodesByVersion"
              ><template #icon><UiIcon name="check" /></template>
                选择此版本
              </n-button>
              <n-button size="small" secondary @click="selectAllEpisodes">全选</n-button>
              <n-button size="small" secondary @click="clearSelectedEpisodes"><template #icon><UiIcon name="close" /></template>清空</n-button>
            </n-space>
          </div>

          <section
            v-if="selectedSeasonGroup"
            class="episode-season-block"
          >
            <div class="episode-season-header">
              <span class="episode-season-title">{{ formatSeasonLabel(selectedSeasonGroup.season_number) }}</span>
              <span class="episode-season-count">{{ selectedSeasonGroup.episodes.length }} 个视频</span>
            </div>

            <div class="episode-parade-grid">
              <div
                v-for="ep in selectedSeasonGroup.episodes"
                :key="ep.emby_id"
                :class="[
                  'episode-chip-shell',
                  {
                    'episode-chip-selected': isEpisodeSelected(ep.emby_id),
                    'episode-chip-multi-version': Number(ep.version_count || 0) > 1,
                    'episode-chip-track-issue': ep.track_status === 'inconsistent'
                  }
                ]"
              >
                <span
                  v-if="ep.track_status === 'inconsistent'"
                  class="episode-chip-issue-dot"
                >!</span>
                <n-checkbox
                  class="episode-chip-checkbox"
                  :checked="isEpisodeSelected(ep.emby_id)"
                  @update:checked="checked => toggleEpisodeSelection(ep.emby_id, checked)"
                />
                <button
                  type="button"
                  :class="['episode-chip', { 'episode-chip-active': String(currentEditMediaId || '') === String(ep.emby_id || '') }]"
                  :title="getEpisodeTooltip(ep)"
                  @click="handleEpisodeChipClick(ep)"
                >
                  {{ formatEpisodeLabel(ep) }}
                </button>
              </div>
            </div>
          </section>
        </div>

        <n-empty v-else description="未找到在库的分集，请确认该剧集已入库" style="padding: 40px 0;" />
      </n-spin>
    </n-modal>

    <!-- 搜索演员模态框 -->
    <n-modal
      v-model:show="showAddActorModal"
      preset="card"
      style="width: 600px"
      :bordered="false"
      size="huge"
      :segmented="{ content: 'soft', footer: 'soft' }"
    ><template #header>搜索并添加演员</template>
      <n-input
        v-model:value="actorSearchQuery"
        placeholder="输入演员名进行搜索..."
        clearable
        @update:value="debouncedSearchActors"
      />
      <n-spin :show="isSearchingActors" style="margin-top: 20px; min-height: 150px;">
        <n-list hoverable clickable>
          <n-list-item v-for="actor in actorSearchResults" :key="actor.id" @click="selectActor(actor)">
            <template #prefix>
              <n-avatar
                :src="getTMDbImageUrl(actor.profile_path, 'w92')"
                :fallback-src="fallbackAvatar"
                size="large"
                object-fit="cover"
              />
            </template>
            <n-thing :title="actor.name">
              <template #description>
                <n-text depth="3" v-if="actor.known_for">
                  代表作: {{ actor.known_for }}
                </n-text>
                <n-text depth="3" v-else>
                  {{ actor.department || '表演' }}
                </n-text>
              </template>
            </n-thing>
            <template #suffix>
              <n-button size="small" type="primary"><template #icon><UiIcon name="check" /></template>选择</n-button>
            </template>
          </n-list-item>
        </n-list>
        <n-empty v-if="!isSearchingActors && actorSearchResults.length === 0 && actorSearchQuery" description="未找到相关人物" style="padding: 20px 0;" />
      </n-spin>
    </n-modal>

    <!-- ★★★ 图像编辑模态框 (Emby 排版风格 - 单行等大卡片) ★★★ -->
    <n-modal
      v-model:show="showImageEditor"
      preset="card"
      style="width: 1100px; max-width: 95vw;"
      :bordered="false"
      size="huge"
    ><template #header>编辑图像</template>
      <div class="emby-image-grid">
        <div v-for="img in imageTypes" :key="img.type" class="emby-image-card">
          <!-- 图片展示区 (统一使用 16:9 比例，图片 contain 缩放) -->
          <div class="emby-card-image-container">
            <n-image
              :src="getDynamicImageUrl(img.embyType, img.type)"
              lazy
              object-fit="contain"
              class="full-image"
              :fallback-src="fallbackAvatar"
            >
              <template #placeholder>
                <div class="image-placeholder">
                  <n-icon :component="ImageIcon" size="40" :depth="3" />
                </div>
              </template>
            </n-image>
          </div>

          <!-- 信息与操作区 -->
          <div class="emby-card-footer">
            <div class="emby-card-title">{{ img.label }}</div>
            <div class="emby-card-actions">
              <n-tooltip trigger="hover" placement="bottom">
                <template #trigger>
                  <n-button text class="emby-action-btn" :aria-label="`搜索${img.label}`" @click="openTmdbSelector(img.type, img.tmdbKey)">
                    <n-icon :component="SearchIcon" size="22" />
                  </n-button>
                </template>
                搜索新图像
              </n-tooltip>

              <n-tooltip trigger="hover" placement="bottom">
                <template #trigger>
                  <n-button text class="emby-action-btn" :aria-label="`上传${img.label}`" @click="triggerFileUpload(img.type)">
                    <n-icon :component="CloudUploadIcon" size="24" />
                  </n-button>
                </template>
                上传图像文件
              </n-tooltip>

              <n-tooltip trigger="hover" placement="bottom">
                <template #trigger>
                  <n-button text class="emby-action-btn" :aria-label="`设置${img.label}网络图片`" @click="openUrlPrompt(img.type)">
                    <n-icon :component="LinkIcon" size="22" />
                  </n-button>
                </template>
                设置网络图像
              </n-tooltip>
            </div>
          </div>
        </div>
      </div>

      <!-- 隐藏的文件上传组件 -->
      <input
        type="file"
        ref="fileInputRef"
        style="display: none"
        accept="image/jpeg, image/png, image/webp"
        @change="handleFileUpload"
      >
    </n-modal>

    <!-- ★★★ TMDb 备选图模态框 (Emby 排版风格 - 完整显示不裁剪) ★★★ -->
    <n-modal
      v-model:show="showTmdbSelector"
      preset="card"
      style="width: 1200px; max-width: 95vw;"
      :bordered="false"
      size="huge"
    ><template #header>{{ `搜索图像 - ${currentTmdbImageLabel}` }}</template>
      <div v-if="isFetchingTmdbImages" class="tmdb-loading-state">
        <n-spin size="large" />
        <div style="margin-top: 16px; color: var(--n-text-color-3);">正在从 TheMovieDb 拉取数据...</div>
      </div>

      <div v-else-if="currentTmdbImages.length === 0" class="tmdb-loading-state">
        <n-empty description="未找到相关图像" />
      </div>

      <div v-else class="emby-tmdb-grid">
        <div
          v-for="(img, index) in currentTmdbImages"
          :key="index"
          class="emby-tmdb-card"
          @click="selectTmdbImage(img.original)"
        >
          <!-- 动态比例容器，内部图片 contain -->
          <div class="tmdb-card-image-wrapper" :style="{ aspectRatio: currentTmdbImageAspect }">
            <n-image
              :src="img.preview"
              lazy
              object-fit="contain"
              preview-disabled
              class="full-image"
            />
          </div>
          <div class="tmdb-card-info">
            <div class="tmdb-meta" v-if="img.width && img.height">
              {{ img.width }} × {{ img.height }}
            </div>
          </div>
        </div>
      </div>
    </n-modal>

    <!-- 输入图片URL模态框 -->
    <n-modal v-model:show="showUrlPrompt" preset="dialog" :show-icon="false"><template #header>设置网络图片</template>
      <n-input
        v-model:value="imageUrlInput"
        type="text"
        placeholder="请粘贴以 http/https 开头的图片直链..."
      />
      <template #action>
        <n-button @click="showUrlPrompt = false">取消</n-button>
        <n-button type="primary" @click="submitUrlImage" :loading="isUploadingImage"><template #icon><UiIcon name="check" /></template>确定</n-button>
      </template>
    </n-modal>
    <!-- ★★★ 媒体信息编辑模态框 ★★★ -->
    <n-modal
      v-model:show="showMediaInfoEditor"
      preset="card"
      style="width: 800px; max-width: 95vw;"
      :bordered="false"
      size="huge"
    ><template #header>编辑媒体信息 (音轨/字幕)</template>
      <n-button secondary :loading="isRevalidatingFingerprints" style="margin-bottom: 16px;"
        :disabled="isSavingMediaInfo || isDeletingMediaVersion || isRepairingTruehdCore"
        @click="revalidateFingerprints(currentEditMediaId)"><template #icon><UiIcon name="check" /></template>
        重新校验当前视频指纹
      </n-button>
      <n-alert v-if="mediaInfoContext.fingerprint_pending" type="warning" style="margin-bottom: 16px;">
        文件身份待校验。请先重新校验指纹，完成后重新打开编辑器，再保存媒体信息。
      </n-alert>
      <n-alert v-if="mediaInfoSaveFeedback" :type="mediaInfoSaveFeedback.type" :title="mediaInfoSaveFeedback.message"
        style="margin-bottom: 16px;">
        <div style="max-height: 200px; overflow: auto; overflow-wrap: anywhere;">
          <div v-for="(detail, index) in mediaInfoSaveFeedback.details" :key="index">{{ detail }}</div>
        </div>
      </n-alert>
      <n-text depth="3" style="display: block; margin-bottom: 16px;">
        保存会重载媒体信息；章节图失效时，请在神医助手中手动重新提取。
      </n-text>
      <n-form-item
        v-if="mediaInfoVersionOptions.length === 1 && mediaInfoSingleVersionFileName"
        label="媒体文件"
        label-placement="left"
      >
        <div
          class="media-info-single-file-name"
          :title="mediaInfoSingleVersionFileName"
        >
          {{ mediaInfoSingleVersionFileName }}
        </div>
      </n-form-item>

      <n-form-item v-if="mediaInfoVersionOptions.length > 1" label="媒体版本" label-placement="left">
        <div class="media-info-version-toolbar">
          <div class="media-info-version-controls">
            <div
              v-if="mediaInfoCurrentVersionFileName"
              class="media-info-current-version-name"
              :title="mediaInfoCurrentVersionFileName"
            >
              <span v-if="mediaInfoCurrentVersionMeta" class="media-info-version-accent">
                {{ mediaInfoCurrentVersionMeta }}
              </span>
              <span class="media-info-version-filename">{{ mediaInfoCurrentVersionFileName }}</span>
            </div>
            <n-select
              class="media-info-version-select"
              :value="currentMediaInfoVersionId"
              :options="mediaInfoVersionOptions"
              :consistent-menu-width="false"
              :render-label="renderMediaInfoVersionLabel"
              @update:value="handleMediaInfoVersionChange"
            />
          </div>
          <n-popconfirm :to="true" placement="bottom-start" style="max-width: min(360px, 70vw)" @positive-click="deleteCurrentMediaVersion">
            <template #trigger>
              <n-button
                type="error"
                secondary
                :loading="isDeletingMediaVersion"
                :disabled="isSavingMediaInfo || isRepairingTruehdCore"
              >
                <template #icon><n-icon :component="TrashIcon" /></template>
                删除当前版本
              </n-button>
            </template>
            将永久删除当前选中的整套媒体版本，包括 115 云端文件、本地 STRM、NFO、图片、媒体信息和 Emby 版本。其他格式版本不受影响，确认继续吗？
          </n-popconfirm>
        </div>
      </n-form-item>

      <n-form-item v-if="mediaInfoSegmentOptions.length > 1" label="电影分段" label-placement="left">
        <n-select
          class="media-info-version-select"
          :value="String(currentEditMediaId || '')"
          :options="mediaInfoSegmentOptions"
          @update:value="handleMediaInfoSegmentChange"
        />
      </n-form-item>

      <n-alert type="info" style="margin-bottom: 16px;">
        修改语言标签后保存，系统将自动覆盖底层指纹文件并通知 Emby 重新加载。<br/>
        <b style="color: var(--ui-error-text);">注意：</b>字幕的标题 (Title) 必须包含 <b>“简体”</b> 或 <b>“繁体”</b> 字样，Emby 才能正确识别并显示为 Chinese Simplified/Traditional。
      </n-alert>

      <n-alert v-if="canBatchApplyMediaInfo" type="warning" style="margin-bottom: 16px;">
        <n-checkbox v-model:checked="applyToSelectedEpisodes">
          同时应用到已选 {{ selectedEpisodeIds.length }} 集（当前正在编辑的集也会保存）
        </n-checkbox>
        <div class="media-info-batch-hint">
          仅修改可唯一匹配的轨道；无法匹配的音轨和字幕保持原值。
        </div>
      </n-alert>

      <!-- ★★★ 硬字幕标记按钮 ★★★ -->
      <n-card size="small" style="margin-bottom: 16px;"><template #header>标记硬字幕 (写入视频流 Title)</template>
        <n-space>
          <n-button @click="setHardcodedSubtitle('简中硬字幕')" secondary>简中</n-button>
          <n-button @click="setHardcodedSubtitle('繁中硬字幕')" secondary>繁中</n-button>
          <n-button @click="setHardcodedSubtitle('简英硬字幕')" secondary>简英</n-button>
          <n-button @click="setHardcodedSubtitle('繁英硬字幕')" secondary>繁英</n-button>
          <n-button @click="clearHardcodedSubtitle()" secondary><template #icon><UiIcon name="close" /></template>清除标记</n-button>
        </n-space>
      </n-card>

      <n-space justify="end" style="margin-bottom: 12px;">
        <n-button secondary @click="fillLanguagesToTitles">
          <template #icon>
            <n-icon :component="TextIcon" />
          </template>
          语言填入标题
        </n-button>
      </n-space>

      <div class="media-info-table-scroll" role="region" aria-label="音轨和字幕编辑表格" tabindex="0">
      <n-table :bordered="true" :single-line="false" size="small">
        <thead>
          <tr>
            <th style="width: 120px;">轨道</th>
            <th style="width: 110px;">比特率</th>
            <th>当前标题 (Title)</th>
            <th style="width: 200px;">修正语言 (Language)</th>
            <th style="width: 80px; text-align: center;">默认</th>
            <th style="width: 64px; text-align: center;">操作</th>
          </tr>
        </thead>
        <tbody>
          <!-- ▼▼▼ 只显示音轨和字幕，隐藏视频流 ▼▼▼ -->
          <tr v-for="stream in displayStreams" :key="getMediaInfoStreamRowKey(stream)">
            <td>
              <n-tag
                :type="stream.Type === 'Audio' ? 'info' : 'success'"
                size="small"
              >
                {{ stream.Type === 'Audio' ? '音轨' : '字幕' }} {{ getStreamTypeOrdinal(stream) }}
              </n-tag>
              <div class="stream-technical-label">{{ getStreamTechnicalLabel(stream) }}</div>
            </td>
            <td>
              {{ stream.Type === 'Audio' ? formatAudioBitRate(stream) : '-' }}
            </td>
            <td>
              <input
                v-model="stream.Title"
                class="media-info-title-input"
                type="text"
                placeholder="无标题"
                autocomplete="off"
                spellcheck="false"
              />
            </td>
            <td>
              <div
                title="双击将当前语言填入标题"
                @dblclick.stop.prevent="handleLanguageDoubleClick(stream)"
              >
                <n-select
                  :ref="instance => setMediaInfoLanguageSelectRef(getMediaInfoStreamRowKey(stream), instance)"
                  :value="getMediaInfoLanguageSelection(stream)"
                  :options="stream.Type === 'Audio' ? audioLanguageOptions : subtitleLanguageOptions"
                  :filter="filterMediaInfoLanguageOption"
                  placeholder="输入语言名称或代码"
                  filterable
                  clearable
                  size="small"
                  :aria-label="`${getMediaInfoStreamDeleteLabel(stream)}修正语言`"
                  @update:value="val => handleLanguageChange(stream, val)"
                />
              </div>
            </td>
            <td style="text-align: center;">
              <n-checkbox
                v-model:checked="stream.IsDefault"
                @update:checked="(val) => handleDefaultChange(stream, val)"
              />
            </td>
            <td style="text-align: center;">
              <n-popconfirm :to="true" placement="top-end" style="max-width: min(360px, 70vw)" @positive-click="removeMediaInfoStream(stream)">
                <template #trigger>
                  <n-button
                    text
                    type="error"
                    :disabled="isSavingMediaInfo || isRepairingTruehdCore"
                    title="删除此轨道"
                  >
                    <template #icon>
                      <n-icon :component="TrashIcon" />
                    </template>
                  </n-button>
                </template>
                确认从当前编辑草稿中删除“{{ getMediaInfoStreamDeleteLabel(stream) }}”吗？只有保存后才会生效。
              </n-popconfirm>
            </td>
          </tr>
          <tr v-if="displayStreams.length === 0">
            <td colspan="6" style="text-align: center; padding: 20px;">
              <n-text depth="3">未解析到媒体流</n-text>
            </td>
          </tr>
        </tbody>
      </n-table>
      </div>

      <template #action>
        <n-space justify="space-between">
          <n-popconfirm :to="true" placement="top-start" style="max-width: min(360px, 70vw)" @positive-click="repairCurrentAudioTracks">
            <template #trigger>
              <n-button
                type="warning"
                secondary
                :loading="isRepairingTruehdCore"
                :disabled="isSavingMediaInfo"
              >
                <template #icon>
                  <n-icon :component="RepairIcon" />
                </template>
                纠正多出音轨
              </n-button>
            </template>
            将丢弃当前未保存修改；存在歧义时会只读探测 CD2，并且只纠正证据唯一的高清音轨兼容核心。是否继续？
          </n-popconfirm>
          <n-space>
            <n-button :disabled="isRepairingTruehdCore" @click="showMediaInfoEditor = false">取消</n-button>
            <n-button
              type="primary"
              :disabled="isRepairingTruehdCore"
              :loading="isSavingMediaInfo"
              @click="saveMediaInfo"
            ><template #icon><UiIcon name="save" /></template>
              保存并刷新
            </n-button>
          </n-space>
        </n-space>
      </template>
    </n-modal>

    <!-- 媒体重命名模态框 -->
    <n-modal
      v-model:show="showRenameEditor"
      preset="card"
      style="width: 760px; max-width: 95vw;"
      :bordered="false"
      size="huge"
    ><template #header>重命名媒体</template>
      <n-alert type="info" style="margin-bottom: 16px;">
        输入新的文件名即可。目录按高级设置中的 MoviePilot 重命名格式生成；源文件扩展名固定，115 文件、STRM 和媒体信息路径会同步更新。
      </n-alert>
      <n-form-item label="新文件名">
        <n-input-group>
          <n-input
            :value="renameCustomName"
            @update:value="handleRenameNameChange"
            placeholder="输入文件名"
            spellcheck="false"
          />
          <n-input :value="renameExtension" readonly style="width: 90px;" />
        </n-input-group>
      </n-form-item>
      <n-form-item label="指定 Part">
        <n-input
          v-model:value="renamePart"
          placeholder="留空表示未指定，例如：花絮、Part 1"
          spellcheck="false"
          @update:value="renamePreview = null"
        />
      </n-form-item>
      <n-form-item v-if="renameVersions.length > 1" label="媒体版本">
        <n-select
          v-model:value="renameTargetId"
          :options="renameVersionOptions"
          @update:value="switchRenameVersion"
        />
      </n-form-item>
      <n-space justify="space-between" align="center" style="margin-bottom: 12px;">
        <n-button secondary :loading="isProbingRename" @click="probeRenameFields">
          <template #icon><n-icon :component="RepairIcon" /></template>
          ffprobe 命名补充并预览
        </n-button>
      </n-space>
      <n-text v-if="Object.keys(renameSupplementFields).length" depth="3" style="display:block; margin-bottom:12px;">
        ffprobe 补充：{{ Object.entries(renameSupplementFields).map(([key, value]) => `${key}=${value}`).join(' · ') }}
      </n-text>
      <div v-if="renamePreview" class="rename-result-panel">
        <div class="rename-result-title">预览结果</div>
        <div class="rename-result-row">
          <span class="rename-result-label">目标文件</span>
          <code>{{ renamePreview.target_name }}</code>
        </div>
        <div v-if="renamePreview.target_relative_path" class="rename-result-row rename-result-tree-row">
          <span class="rename-result-label">目标相对路径</span>
          <div class="rename-result-tree">
            <div
              v-for="(part, index) in renamePreviewPathParts"
              :key="`${part}-${index}`"
              class="rename-result-tree-node"
              :style="{ paddingLeft: `${index * 18}px` }"
            >
              <n-icon :component="index === renamePreviewPathParts.length - 1 ? FileIcon : FolderIcon" :color="index === renamePreviewPathParts.length - 1 ? 'var(--ui-info-text)' : 'var(--ui-warning-text)'" />
              <code>{{ part }}</code>
            </div>
          </div>
        </div>
        <div v-if="renamePreview.target_cloud_path" class="rename-result-row">
          <span class="rename-result-label">115 路径</span>
          <code>{{ renamePreview.target_cloud_path }}</code>
        </div>
        <div v-if="renamePreview.target_local_path" class="rename-result-row">
          <span class="rename-result-label">本地路径</span>
          <code>{{ renamePreview.target_local_path }}</code>
        </div>
      </div>
      <template #action>
        <n-space justify="end">
          <n-button @click="previewRename" :loading="isPreviewingRename"><template #icon><UiIcon name="preview" /></template>预览</n-button>
          <n-button
            type="primary"
            @click="executeRename"
            :loading="isExecutingRename"
            :disabled="!renamePreview"
          ><template #icon><UiIcon name="save" /></template>
            保存重命名
          </n-button>
        </n-space>
      </template>
    </n-modal>
  </n-layout>
</template>

<script setup>
import { ref, shallowRef, onMounted, onBeforeUnmount, watch, computed, nextTick, toRaw, h } from 'vue';
import draggable from 'vuedraggable';
import { NIcon, NInput, NInputGroup, NGrid, NGridItem, NFormItem, NTag, NAvatar, NPopconfirm, NImage, NModal, NList, NListItem, NThing, NEmpty, NButtonGroup, NDropdown, NTooltip, NCheckbox, NText, NSelect } from 'naive-ui';
import { useRoute, useRouter } from 'vue-router';
import axios from 'axios';
import { NPageHeader, NDivider, NSpin, NCard, NDescriptions, NDescriptionsItem, NButton, NSpace, NAlert, useMessage } from 'naive-ui';
import {
  MoveOutline as DragHandleIcon,
  TrashOutline as TrashIcon,
  ImageOutline as ImageIcon,
  ImagesOutline as ImagesIcon,
  PersonOutline as PersonIcon,
  AddOutline as AddIcon,
  ChevronDownOutline as ChevronDownIcon,
  CloudUploadOutline as CloudUploadIcon,
  LinkOutline as LinkIcon,
  SearchOutline as SearchIcon,
  DocumentTextOutline as DocumentTextIcon,
  DocumentTextOutline as FileIcon,
  FolderOutline as FolderIcon,
  CloudOutline as CloudIcon,
  BuildOutline as RepairIcon,
  TextOutline as TextIcon
} from '@vicons/ionicons5';
import { debounce } from 'lodash-es';
import { textColorForBackground } from '../theme.js';
import {
  buildMediaInfoLanguageOptions,
  filterMediaInfoLanguageOption,
  findMediaInfoLanguageOption,
  getMediaInfoLanguageAliases,
  getMediaInfoLanguageTitle,
  getMediaInfoLanguageTokens,
  isChineseSubtitleStream,
  normalizeMediaInfoCodeToken,
  resolveMediaInfoLanguageSelection,
  isInvalidAudioProbeStream,
  updateMediaInfoStreamLanguage,
} from '../utils/mediaStreams.js';

const route = useRoute();
const router = useRouter();
const message = useMessage();

const itemId = ref(null);
const isLoading = ref(true);
const itemDetails = ref(null);
const editableCast = ref([]);
const isSaving = ref(false);

const searchLinks = ref({});
const isTranslating = ref(false);

const showAddActorModal = ref(false);
const actorSearchQuery = ref('');
const actorSearchResults = ref([]);
const isSearchingActors = ref(false);
const fallbackAvatar = 'data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7';

// ★★★ 图像编辑相关状态 ★★★
const showImageEditor = ref(false);
const showUrlPrompt = ref(false);
const imageUrlInput = ref('');
const currentEditImageType = ref('');
const fileInputRef = ref(null);
const isUploadingImage = ref(false);

// 用于强制刷新图片的随机时间戳
const imageRefreshTokens = ref({
  poster: Date.now(),
  clearlogo: Date.now(),
  fanart: Date.now(),
  landscape: Date.now()
});

const imageTypes = [
  { type: 'poster', label: '海报 (Poster)', embyType: 'Primary', aspect: '2/3', tmdbKey: 'posters' },
  { type: 'clearlogo', label: '标志 (Logo)', embyType: 'Logo', aspect: '16/9', tmdbKey: 'logos' },
  { type: 'fanart', label: '艺术图 (Fanart)', embyType: 'Backdrop', aspect: '16/9', tmdbKey: 'backdrops' },
  { type: 'landscape', label: '缩略图 (Landscape)', embyType: 'Thumb', aspect: '16/9', tmdbKey: 'backdrops' } // 缩略图通常用背景图代替
];

// 动态获取图片URL (带时间戳防缓存)
const getDynamicImageUrl = (embyType, typeKey) => {
  if (!itemDetails.value?.item_id) return '';
  // 如果是 Thumb，Emby 有时没有 Thumb 会用 Backdrop 代替，这里统一请求
  return `/image_proxy/Items/${itemDetails.value.item_id}/Images/${embyType}?quality=90&t=${imageRefreshTokens.value[typeKey]}`;
};

const posterUrl = computed(() => {
  return getDynamicImageUrl('Primary', 'poster');
});

// ★★★ 图像上传逻辑 ★★★
const triggerFileUpload = (type) => {
  currentEditImageType.value = type;
  if (fileInputRef.value) {
    fileInputRef.value.click();
  }
};

const handleFileUpload = async (event) => {
  const file = event.target.files[0];
  if (!file) return;

  const formData = new FormData();
  formData.append('image_type', currentEditImageType.value);
  formData.append('file', file);

  await uploadImagePayload(formData);
  // 清空 input 以便下次选择同名文件
  event.target.value = '';
};

const openUrlPrompt = (type) => {
  currentEditImageType.value = type;
  imageUrlInput.value = '';
  showUrlPrompt.value = true;
};

const submitUrlImage = async () => {
  if (!imageUrlInput.value.startsWith('http')) {
    message.error("请输入有效的 http/https 图片链接");
    return;
  }

  const payload = {
    image_type: currentEditImageType.value,
    image_url: imageUrlInput.value
  };

  await uploadImagePayload(payload);
  showUrlPrompt.value = false;
};

const uploadImagePayload = async (payload) => {
  isUploadingImage.value = true;
  const loadingMsg = message.loading("正在替换并通知 Emby 刷新...", { duration: 0 });

  try {
    const config = payload instanceof FormData
      ? { headers: { 'Content-Type': 'multipart/form-data' } }
      : {};

    const res = await axios.post(`/api/update_media_image/${itemId.value}`, payload, config);
    message.success(res.data.message || "图片替换成功！");

    // 刷新对应图片的时间戳，强制前端重新加载图片
    imageRefreshTokens.value[currentEditImageType.value] = Date.now();

  } catch (error) {
    console.error("图片替换失败:", error);
    message.error(error.response?.data?.error || "图片替换失败，请检查后端日志。");
  } finally {
    loadingMsg.destroy();
    isUploadingImage.value = false;
  }
};

const replaceEpisodeImages = async () => {
  isReplacingEpisodeImages.value = true;
  const loadingMsg = message.loading('剧集图正在后台处理，可在通知中心中查看生成数量...', { duration: 0 });
  try {
    const response = await axios.post(`/api/media_info/replace_episode_images/${itemId.value}`);
    message.success(
      `${response.data.message || '剧集图替换请求已完成'}；已有图删除 ${response.data.deleted_count || 0} 集，原本无图 ${response.data.missing_count || 0} 集。`,
    );
  } catch (error) {
    message.error(error.response?.data?.error || '替换剧集图失败，请检查日志。');
  } finally {
    loadingMsg.destroy();
    isReplacingEpisodeImages.value = false;
  }
};
// ★★★ 图像编辑逻辑结束 ★★★

// =========================================================
// ★★★ 媒体信息 (MediaInfo) 编辑相关状态与逻辑 ★★★
// =========================================================
const showMediaInfoEditor = ref(false);
const isSavingMediaInfo = ref(false);
const isDeletingMediaVersion = ref(false);
const isRepairingTruehdCore = ref(false);
const isRepairingP115Orphan = ref(false);
const isRevalidatingFingerprints = ref(false);
const fingerprintScopeLabel = computed(() => ({
  Movie: '当前电影', Series: '整部电视剧', Season: '当前季', Episode: '当前集'
}[itemDetails.value?.item_type] || '当前媒体'));
const revalidateFingerprints = async (id) => {
  const targetId = String(id || '').trim();
  if (!targetId) return message.error('无法确定当前媒体 Item ID');
  isRevalidatingFingerprints.value = true;
  try {
    const response = await axios.post(`/api/media_info/revalidate_fingerprints/${encodeURIComponent(targetId)}`);
    message.success(response.data.message);
  } catch (error) {
    message.error(error.response?.data?.error || '提交指纹校验失败');
  } finally {
    isRevalidatingFingerprints.value = false;
  }
};
const isReplacingEpisodeImages = ref(false);
const showRenameEditor = ref(false);
const isProbingRename = ref(false);
const isPreviewingRename = ref(false);
const isExecutingRename = ref(false);
const renameCustomName = ref('');
const renameExtension = ref('');
const renamePart = ref('');
const renameUseFfprobe = ref(false);
const renameFields = ref({});
const renameSupplementFields = ref({});
const renamePreview = ref(null);
const renamePreviewPathParts = computed(() => String(renamePreview.value?.target_relative_path || '').split('/').filter(Boolean));
const renameTargetId = ref(null);
const renameVersions = ref([]);
const p115OrphanScopeLabel = computed(() => {
  const itemType = String(itemDetails.value?.item_type || '');
  if (itemType === 'Series') return '当前剧集下的分集';
  if (itemType === 'Season') return '当前季的分集';
  return '当前媒体版本';
});
const renameVersionOptions = computed(() => renameVersions.value.map((version) => ({
  label: version.label || `版本 ${version.emby_id}`,
  value: String(version.emby_id),
})));
const mediaInfoData = shallowRef(null);
const mediaStreams = ref([]);
const mediaInfoContext = ref({});
const mediaInfoSaveFeedback = ref(null);
const currentEditMediaId = ref(null); // ★ 新增：记录当前正在编辑的具体媒体ID (电影ID或具体的集ID)
const mediaInfoVersions = ref([]);
let mediaInfoStreamSnapshotMap = new WeakMap();
const deletedMediaStreamSnapshots = ref([]);
const mediaInfoLanguageSelections = ref({});
const mediaInfoIsoLanguageOptions = ref([]);
const subtitleChineseDefaultLanguage = ref('chs');
const mediaInfoLanguageSelectRefs = new Map();
const mediaInfoLanguageUndoStack = [];

// ★ 新增：剧集分集选择相关状态
const showEpisodeSelector = ref(false);
const isFetchingEpisodes = ref(false);
const episodesList = ref([]);
const selectedEpisodeIds = ref([]);
const applyToSelectedEpisodes = ref(false);
const selectedSeasonNumber = ref(null);
const selectedBatchVersionIndex = ref(null);

const canBatchApplyMediaInfo = computed(() => selectedEpisodeIds.value.length > 0);
const mediaInfoVersionOptions = computed(() => mediaInfoVersions.value.map((version) => ({
  ...version,
  label: version.label || version.display_name || `版本 ${version.emby_id}`,
  value: String(version.emby_id)
})));
const mediaInfoVersionMeta = (version) => {
  const parts = [];
  if (version?.version_number) parts.push(`版本 ${version.version_number}`);
  if (version?.format) parts.push(String(version.format));
  if (version?.version_detail) parts.push(String(version.version_detail));
  return parts.join(' · ');
};
const renderMediaInfoVersionLabel = (option) => h(
  'div',
  { class: 'media-info-version-option-label' },
  [
    h('span', { class: 'media-info-version-accent' }, mediaInfoVersionMeta(option)),
    h('span', { class: 'media-info-version-filename' }, option.file_name || option.display_name || option.label),
  ],
);
const currentMediaInfoVersion = computed(() => mediaInfoVersions.value.find(version => (
  String(version.emby_id) === String(currentEditMediaId.value || '')
  || (version.segments || []).some(segment => String(segment.emby_id) === String(currentEditMediaId.value || ''))
)) || null);
const currentMediaInfoVersionId = computed(() => String(
  currentMediaInfoVersion.value?.emby_id || currentEditMediaId.value || ''
));
const mediaInfoSingleVersionFileName = computed(() => {
  if (mediaInfoVersionOptions.value.length !== 1) return '';
  const version = mediaInfoVersions.value[0] || {};
  if (version.file_name || version.display_name) {
    return String(version.file_name || version.display_name);
  }
  return String(version.path || '').split(/[\\/]/).pop() || '';
});
const mediaInfoCurrentVersionFileName = computed(() => {
  const version = currentMediaInfoVersion.value || {};
  if (version.file_name || version.display_name) {
    return String(version.file_name || version.display_name);
  }
  return String(version.path || '').split(/[\\/]/).pop() || '';
});
const mediaInfoCurrentVersionMeta = computed(() => mediaInfoVersionMeta(currentMediaInfoVersion.value || {}));
const mediaInfoSegmentOptions = computed(() => (currentMediaInfoVersion.value?.segments || []).map(segment => ({
  label: segment.label,
  value: String(segment.emby_id)
})));

const openRenameEditor = async () => {
  const loadingMsg = message.loading('正在读取重命名上下文...', { duration: 0 });
  const targetId = String(currentEditMediaId.value || itemId.value || '').trim();
  try {
    const res = await axios.get(`/api/media_rename/${targetId}`);
    renameTargetId.value = targetId;
    renameExtension.value = res.data.file_ext || '';
    renameCustomName.value = stripRenameExtension(res.data.source_name || '');
    renamePart.value = '';
    renameUseFfprobe.value = false;
    renameFields.value = res.data.fields || {};
    renameSupplementFields.value = {};
    renamePreview.value = null;
    renameVersions.value = Array.isArray(res.data.versions) ? res.data.versions : [];
    showRenameEditor.value = true;
  } catch (error) {
    message.error(error.response?.data?.error || '读取重命名上下文失败');
  } finally {
    loadingMsg.destroy();
  }
};

const switchRenameVersion = async (targetId) => {
  try {
    const res = await axios.get(`/api/media_rename/${targetId}`);
    renameTargetId.value = String(targetId);
    renameExtension.value = res.data.file_ext || renameExtension.value;
    renameCustomName.value = stripRenameExtension(res.data.source_name || '');
    renamePart.value = '';
    renameFields.value = res.data.fields || {};
    renameSupplementFields.value = {};
    renameUseFfprobe.value = false;
    renamePreview.value = null;
  } catch (error) {
    message.error(error.response?.data?.error || '切换媒体版本失败');
  }
};

const handleRenameNameChange = (value) => {
  renameCustomName.value = stripRenameExtension(value);
  renamePreview.value = null;
};

const stripRenameExtension = (value) => {
  const text = String(value || '').trim();
  const extension = String(renameExtension.value || '').trim();
  if (extension && text.toLowerCase().endsWith(extension.toLowerCase())) {
    return text.slice(0, -extension.length);
  }
  return text;
};

const renameFullName = () => `${renameCustomName.value.trim()}${renameExtension.value}`;

const renamePayload = () => ({
  mode: 'custom',
  custom_name: renameFullName(),
  use_ffprobe: renameUseFfprobe.value,
  fields: { ...renameFields.value, part: renamePart.value.trim() },
  preview_token: renamePreview.value?.preview_token || '',
});

const getRenameTargetId = () => String(renameTargetId.value || itemId.value || '').trim();

const probeRenameFields = async () => {
  isProbingRename.value = true;
  renamePreview.value = null;
  try {
    const res = await axios.post(`/api/media_rename/${getRenameTargetId()}/ffprobe`, {
      custom_name: renameFullName(),
    });
    renameSupplementFields.value = res.data.fields || {};
    renameCustomName.value = stripRenameExtension(res.data.suggested_name || renameFullName());
    renameFields.value = Object.fromEntries(Object.entries({
      ...renameFields.value,
      ...Object.fromEntries(Object.entries(renameSupplementFields.value).filter(([key]) => !renameFields.value[key])),
    }));
    renameUseFfprobe.value = false;
    if (!Object.keys(renameSupplementFields.value).length) {
      message.warning('未读取到可用的本地媒体流信息');
    } else {
      message.success('ffprobe 补充字段已载入');
      await previewRename();
    }
  } catch (error) {
    message.error(error.response?.data?.error || 'ffprobe 命名补充失败');
  } finally {
    isProbingRename.value = false;
  }
};

const previewRename = async () => {
  isPreviewingRename.value = true;
  try {
    const res = await axios.post(`/api/media_rename/${getRenameTargetId()}/preview`, renamePayload());
    renamePreview.value = res.data;
  } catch (error) {
    renamePreview.value = null;
    message.error(error.response?.data?.error || '预览重命名失败');
  } finally {
    isPreviewingRename.value = false;
  }
};

const executeRename = async () => {
  if (!renamePreview.value) {
    message.warning('请先预览并确认重命名结果');
    return;
  }
  isExecutingRename.value = true;
  try {
    const res = await axios.post(`/api/media_rename/${getRenameTargetId()}/execute`, renamePayload());
    message.success(res.data.message || '媒体已重命名');
    showRenameEditor.value = false;
    const newItemId = String(res.data.item_id || '').trim();
    if (newItemId && newItemId !== String(itemId.value || '')) {
      itemId.value = newItemId;
      currentEditMediaId.value = newItemId;
      renameTargetId.value = newItemId;
      await router.replace({ name: 'MediaEditPage', params: { itemId: newItemId } });
    }
    await fetchMediaDetails();
  } catch (error) {
    message.error(error.response?.data?.error || '执行重命名失败，请检查日志');
  } finally {
    isExecutingRename.value = false;
  }
};

const repairCurrentP115Orphans = async () => {
  const targetId = String(itemDetails.value?.item_id || itemId.value || '').trim();
  if (!targetId) {
    message.error('无法确定当前媒体 Item ID');
    return;
  }
  isRepairingP115Orphan.value = true;
  try {
    const response = await axios.post(`/api/media_info/repair_p115_orphan/${targetId}`);
    message.success(response.data?.message || '当前媒体的115孤儿文件检查已提交');
  } catch (error) {
    message.error(error.response?.data?.error || '提交115孤儿文件检查失败');
  } finally {
    isRepairingP115Orphan.value = false;
  }
};

const editableStreamFields = {
  Video: ['Title'],
  Audio: ['Title', 'Language', 'IsDefault'],
  Subtitle: ['Title', 'Language', 'IsDefault', 'IsForced']
};

const createMediaStreamSnapshots = (streams) => {
  const typeOrdinals = { Video: 0, Audio: 0, Subtitle: 0 };
  return streams.map((stream) => {
    typeOrdinals[stream.Type] = (typeOrdinals[stream.Type] || 0) + 1;
    const fields = editableStreamFields[stream.Type] || [];
    return {
      identity: {
        type: stream.Type,
        index: stream.Index ?? null,
        ordinal: typeOrdinals[stream.Type],
        codec: stream.Codec || null,
        channels: stream.Type === 'Audio' ? (stream.Channels ?? null) : null,
        is_external: stream.Type === 'Subtitle' ? Boolean(stream.IsExternal) : null,
        language: stream.Language ?? null,
        title: stream.Title ?? null
      },
      values: Object.fromEntries(fields.map(field => [field, stream[field]]))
    };
  });
};

const streamValuesEqual = (left, right) => left === right || (left == null && right == null);

const buildStreamPatches = () => {
  const editPatches = mediaStreams.value.flatMap((stream) => {
    const snapshot = mediaInfoStreamSnapshotMap.get(stream);
    if (!snapshot || snapshot.identity.type !== stream.Type) return [];
    const changes = {};
    for (const field of editableStreamFields[stream.Type] || []) {
      const currentValue = stream[field];
      if (!streamValuesEqual(snapshot.values[field], currentValue)) {
        changes[field] = currentValue ?? null;
      }
    }
    return Object.keys(changes).length > 0
      ? [{ identity: snapshot.identity, changes }]
      : [];
  });
  const deletePatches = deletedMediaStreamSnapshots.value.map(snapshot => ({
    identity: snapshot.identity,
    delete: true,
  }));
  return [...editPatches, ...deletePatches];
};

const getStreamTypeOrdinal = (stream) => {
  const sameTypeStreams = mediaStreams.value.filter(item => item.Type === stream.Type);
  return sameTypeStreams.indexOf(stream) + 1;
};

const getStreamTechnicalLabel = (stream) => {
  const parts = [];
  if (stream.Index !== undefined && stream.Index !== null) parts.push(`#${stream.Index}`);
  if (stream.Codec) parts.push(String(stream.Codec).toUpperCase());
  if (stream.Type === 'Audio' && stream.Channels) parts.push(`${stream.Channels}ch`);
  if (stream.Type === 'Subtitle' && stream.IsExternal) parts.push('外挂');
  return parts.join(' · ') || '未标识';
};

const formatAudioBitRate = (stream) => {
  const rawValue = stream?.BitRate ?? stream?.Bitrate ?? stream?.bit_rate ?? stream?.bitrate;
  const bitRate = Number(rawValue);
  if (!Number.isFinite(bitRate) || bitRate <= 0) return '未知';
  return `${Math.round(bitRate / 1000).toLocaleString('zh-CN')} kbps`;
};

const getMediaInfoSourceStreams = () => {
  if (Array.isArray(mediaInfoData.value) && mediaInfoData.value.length > 0) {
    const source = mediaInfoData.value[0]?.MediaSourceInfo || mediaInfoData.value[0];
    return Array.isArray(source?.MediaStreams) ? source.MediaStreams : null;
  }
  return Array.isArray(mediaInfoData.value?.MediaStreams) ? mediaInfoData.value.MediaStreams : null;
};

const getMediaInfoStreamDeleteLabel = (stream) => {
  const typeLabel = stream.Type === 'Audio' ? '音轨' : '字幕';
  return `${typeLabel} ${getStreamTypeOrdinal(stream)} · ${getStreamTechnicalLabel(stream)}`;
};

const removeMediaInfoStream = (stream) => {
  const sourceStreams = getMediaInfoSourceStreams();
  const sourceIndex = sourceStreams?.indexOf(toRaw(stream)) ?? -1;
  if (sourceIndex < 0) {
    message.error('无法定位该轨道，未删除');
    return;
  }

  const selectionKey = getMediaInfoStreamSelectionKey(stream);
  const selectionsByStream = new Map(
    mediaStreams.value
      .filter(item => item.Type === 'Audio' || item.Type === 'Subtitle')
      .map(item => [item, getMediaInfoLanguageSelection(item)])
  );
  const snapshot = mediaInfoStreamSnapshotMap.get(stream);
  sourceStreams.splice(sourceIndex, 1);
  mediaStreams.value = mediaStreams.value.filter(item => item !== stream);
  if (snapshot && !deletedMediaStreamSnapshots.value.includes(snapshot)) {
    deletedMediaStreamSnapshots.value = [...deletedMediaStreamSnapshots.value, snapshot];
  }
  mediaInfoLanguageSelections.value = Object.fromEntries(
    mediaStreams.value
      .filter(item => item.Type === 'Audio' || item.Type === 'Subtitle')
      .map(item => [getMediaInfoStreamSelectionKey(item), selectionsByStream.get(item) ?? null])
  );
  for (let index = mediaInfoLanguageUndoStack.length - 1; index >= 0; index -= 1) {
    if (mediaInfoLanguageUndoStack[index].selectionKey === selectionKey) {
      mediaInfoLanguageUndoStack.splice(index, 1);
    }
  }
  message.info('已从编辑草稿移除，保存后才会真正删除');
};

// ★★★ 剧集选择：按季分组，做成“阅兵式”方阵 ★★★
const episodeSeasonGroups = computed(() => {
  const groupMap = new Map();

  episodesList.value.forEach((ep) => {
    const seasonNo = Number(ep.season_number ?? 0);
    const safeSeasonNo = Number.isFinite(seasonNo) ? seasonNo : 0;

    if (!groupMap.has(safeSeasonNo)) {
      groupMap.set(safeSeasonNo, []);
    }
    groupMap.get(safeSeasonNo).push(ep);
  });

  return Array.from(groupMap.entries())
    .sort(([a], [b]) => a - b)
    .map(([season_number, episodes]) => ({
      season_number,
      episodes: episodes.slice().sort((a, b) => {
        const aNo = Number(a.episode_number ?? 0);
        const bNo = Number(b.episode_number ?? 0);
        return (Number.isFinite(aNo) ? aNo : 0) - (Number.isFinite(bNo) ? bNo : 0);
      })
    }));
});

const selectedSeasonGroup = computed(() => episodeSeasonGroups.value.find(
  season => season.season_number === selectedSeasonNumber.value
) || null);

const visibleSeasonEpisodes = computed(() => selectedSeasonGroup.value?.episodes || []);
const batchVersionOptions = computed(() => {
  const counts = new Map();
  visibleSeasonEpisodes.value.forEach((ep) => {
    const versionIndex = Number(ep.version_index || 1);
    const current = counts.get(versionIndex) || { count: 0, summary: ep.version_summary || '' };
    current.count += 1;
    if (!current.summary && ep.version_summary) current.summary = ep.version_summary;
    counts.set(versionIndex, current);
  });
  return Array.from(counts.entries())
    .sort(([left], [right]) => left - right)
    .map(([versionIndex, version]) => ({
      label: `版本 ${versionIndex}${version.summary ? ` · ${version.summary}` : ''}（${version.count} 个视频）`,
      value: versionIndex
    }));
});
const currentSeasonHasMultipleVersions = computed(() => visibleSeasonEpisodes.value.some(
  ep => Number(ep.season_version_count || 0) > 1
));

const formatSeasonLabel = (seasonNumber) => {
  const n = Number(seasonNumber);
  if (n === 0) return '特别篇';
  return `第 ${Number.isFinite(n) ? n : seasonNumber} 季`;
};

const formatEpisodeLabel = (ep) => {
  const n = Number(ep?.episode_number);
  if (!Number.isFinite(n)) return ep?.episode_number || '?';
  const episodeLabel = String(n).padStart(2, '0');
  return Number(ep?.season_version_count || 0) > 1
    ? `${episodeLabel}.${ep?.version_index || 1}`
    : episodeLabel;
};

const getEpisodeTooltip = (ep) => {
  const episodeNo = ep?.episode_number ?? '?';
  const title = ep?.title || '未知标题';
  const version = ep?.version_label ? `；${ep.version_label}` : '';
  const trackIssue = ep?.track_difference ? `；轨道提示：${ep.track_difference}` : '';
  return `${formatSeasonLabel(ep?.season_number)} 第 ${episodeNo} 集：${title}${version}${trackIssue}`;
};

const handleEpisodeChipClick = (ep) => {
  // 不再关闭分集选择框：媒体信息编辑框会盖在上层，关掉后还能继续点下一集。
  openMediaInfoEditor(ep.emby_id);
};

const isEpisodeSelected = (embyId) => {
  return selectedEpisodeIds.value.includes(String(embyId || ''));
};

const toggleEpisodeSelection = (embyId, checked) => {
  const targetId = String(embyId || '').trim();
  if (!targetId) return;
  if (checked && !selectedEpisodeIds.value.includes(targetId)) {
    selectedEpisodeIds.value = [...selectedEpisodeIds.value, targetId];
    applyToSelectedEpisodes.value = true;
  } else if (!checked) {
    selectedEpisodeIds.value = selectedEpisodeIds.value.filter(id => id !== targetId);
    if (selectedEpisodeIds.value.length === 0) {
      applyToSelectedEpisodes.value = false;
    }
  }
};

const handleSeasonChange = (seasonNumber) => {
  selectedSeasonNumber.value = seasonNumber;
  selectedBatchVersionIndex.value = null;
  clearSelectedEpisodes();
};

const selectAllEpisodes = () => {
  selectedEpisodeIds.value = visibleSeasonEpisodes.value
    .map(ep => String(ep.emby_id || '').trim())
    .filter(Boolean);
  applyToSelectedEpisodes.value = selectedEpisodeIds.value.length > 0;
};

const selectEpisodesByVersion = () => {
  if (selectedBatchVersionIndex.value === null) return;
  selectedEpisodeIds.value = visibleSeasonEpisodes.value
    .filter(ep => Number(ep.version_index || 1) === Number(selectedBatchVersionIndex.value))
    .map(ep => String(ep.emby_id || '').trim())
    .filter(Boolean);
  applyToSelectedEpisodes.value = selectedEpisodeIds.value.length > 0;
};

const clearSelectedEpisodes = () => {
  selectedEpisodeIds.value = [];
  applyToSelectedEpisodes.value = false;
};

// ★★★ 拆分音轨和字幕的下拉选项 ★★★
const audioLanguageOptions = ref([]);
const subtitleLanguageOptions = ref([]);


// ★ 新增：点击编辑按钮的分流逻辑
const handleMediaInfoButtonClick = async () => {
  if (['Series', 'Season'].includes(itemDetails.value.item_type)) {
    // 如果是剧集或季，先拉取分集列表并打开选择框
    showEpisodeSelector.value = true;
    isFetchingEpisodes.value = true;
    clearSelectedEpisodes();
    try {
      const res = await axios.get(`/api/media_info/series/${itemId.value}/episodes`);
      episodesList.value = res.data;
      selectedSeasonNumber.value = episodeSeasonGroups.value[0]?.season_number ?? null;
      selectedBatchVersionIndex.value = null;
    } catch (e) {
      message.error(e.response?.data?.error || "获取分集列表失败");
      showEpisodeSelector.value = false;
    } finally {
      isFetchingEpisodes.value = false;
    }
  } else {
    // 如果是电影，直接打开编辑器
    openMediaInfoEditor(itemId.value);
  }
};
// 1. 获取语言映射表
const getFallbackMediaInfoLanguageLabel = (language) => {
  const token = normalizeMediaInfoCodeToken(language);
  const knownLabels = {
    rom: '罗姆语',
    gem: '日耳曼语族',
    mul: '多语言',
    mis: '未分类语言',
    zxx: '无语言内容',
    und: '无语言',
  };
  if (knownLabels[token]) return knownLabels[token];
  try {
    const displayNames = new Intl.DisplayNames(['zh-CN'], { type: 'language' });
    return displayNames.of(token) || token;
  } catch (error) {
    return token;
  }
};

const ensureMediaInfoLanguageOption = (stream) => {
  if (!['Audio', 'Subtitle'].includes(stream?.Type)) return;
  const language = String(stream?.Language || '').trim();
  if (!language) return;
  const languageToken = normalizeMediaInfoCodeToken(language);
  const options = stream.Type === 'Audio' ? audioLanguageOptions.value : subtitleLanguageOptions.value;
  if (findMediaInfoLanguageOption(options, language)) return;
  if (
    stream.Type === 'Subtitle'
    && (languageToken === 'yue' || languageToken.startsWith('yue-') || languageToken === 'zh-yue')
  ) {
    return;
  }
  const isoEntry = mediaInfoIsoLanguageOptions.value.find(item => {
    const itemTokens = getMediaInfoLanguageAliases(item);
    return getMediaInfoLanguageTokens(language).some(token => itemTokens.includes(token));
  });
  const fallbackLabel = getFallbackMediaInfoLanguageLabel(language);
  // 与自动填充共用后端中文名称，避免浏览器与服务端各用一套语言标签。
  const label = isoEntry?.label || fallbackLabel || language;
  options.push({
    label,
    value: language,
    _languageTokens: isoEntry
      ? getMediaInfoLanguageAliases(isoEntry)
      : getMediaInfoLanguageTokens(language),
  });
  console.info(`[媒体信息编辑] 已通过 ISO 639 加入语言选项: ${language} -> ${label}`);
};

const fetchLanguageMapping = async () => {
  try {
    const res = await axios.get('/api/mappings/config/language_mapping');
    const rawData = Array.isArray(res.data) ? res.data : [];
    audioLanguageOptions.value = buildMediaInfoLanguageOptions(rawData);
    subtitleLanguageOptions.value = buildMediaInfoLanguageOptions(rawData, true);
  } catch (e) {
    console.error("获取语言映射失败", e);
  }
};
const stripChineseScriptParenthetical = (value) => String(value || '')
  .replace(/\s*[（(]\s*(?:简体|繁体|簡體|繁體|简中|繁中)(?:中文|字)?\s*[）)]/gi, '')
  .trim();

const getMediaInfoStreamSelectionKey = (stream) => {
  const streamIndex = stream?.Index ?? getStreamTypeOrdinal(stream);
  return `${stream?.Type || 'Unknown'}:${streamIndex}`;
};

const getMediaInfoStreamRowKey = (stream) => {
  const ordinal = getStreamTypeOrdinal(stream);
  const streamIndex = stream?.Index ?? 'none';
  return `${currentEditMediaId.value || 'unknown'}:${stream?.Type || 'Unknown'}:${streamIndex}:${ordinal}`;
};

const setMediaInfoLanguageSelectRef = (key, instance) => {
  if (instance) mediaInfoLanguageSelectRefs.set(key, instance);
  else mediaInfoLanguageSelectRefs.delete(key);
};

const resolveInitialMediaInfoLanguageSelection = (stream) => {
  if (!stream) return null;
  const options = stream.Type === 'Audio' ? audioLanguageOptions.value : subtitleLanguageOptions.value;
  return resolveMediaInfoLanguageSelection(
    options,
    stream,
    subtitleChineseDefaultLanguage.value,
  );
};

const getMediaInfoLanguageSelection = (stream) => {
  const key = getMediaInfoStreamSelectionKey(stream);
  if (Object.hasOwn(mediaInfoLanguageSelections.value, key)) {
    return mediaInfoLanguageSelections.value[key];
  }
  return resolveInitialMediaInfoLanguageSelection(stream);
};

const getMediaInfoLanguageOption = (stream) => {
  const options = stream.Type === 'Audio' ? audioLanguageOptions.value : subtitleLanguageOptions.value;
  const selectedValue = getMediaInfoLanguageSelection(stream);
  return options.find(option => option.value === selectedValue)
    || findMediaInfoLanguageOption(options, stream.Language);
};

const recordMediaInfoLanguageUndo = (stream) => {
  const selectionKey = getMediaInfoStreamSelectionKey(stream);
  mediaInfoLanguageUndoStack.push({
    targetId: String(currentEditMediaId.value || ''),
    selectionKey,
    hadSelection: Object.hasOwn(mediaInfoLanguageSelections.value, selectionKey),
    selection: getMediaInfoLanguageSelection(stream),
    hadLanguage: Object.hasOwn(stream, 'Language'),
    language: stream.Language,
    hadTitle: Object.hasOwn(stream, 'Title'),
    title: stream.Title,
  });
  if (mediaInfoLanguageUndoStack.length > 50) {
    mediaInfoLanguageUndoStack.shift();
  }
};

const restoreMediaInfoProperty = (stream, field, existed, value) => {
  if (existed) stream[field] = value;
  else delete stream[field];
};

const undoLastMediaInfoLanguageChange = () => {
  const currentTargetId = String(currentEditMediaId.value || '');
  let undoEntry = null;
  while (mediaInfoLanguageUndoStack.length > 0 && !undoEntry) {
    const candidate = mediaInfoLanguageUndoStack.pop();
    if (candidate.targetId === currentTargetId) undoEntry = candidate;
  }
  if (!undoEntry) return false;

  const stream = mediaStreams.value.find(item => (
    getMediaInfoStreamSelectionKey(item) === undoEntry.selectionKey
  ));
  if (!stream) return false;

  restoreMediaInfoProperty(stream, 'Language', undoEntry.hadLanguage, undoEntry.language);
  restoreMediaInfoProperty(stream, 'Title', undoEntry.hadTitle, undoEntry.title);

  const selections = { ...mediaInfoLanguageSelections.value };
  if (undoEntry.hadSelection) selections[undoEntry.selectionKey] = undoEntry.selection;
  else delete selections[undoEntry.selectionKey];
  mediaInfoLanguageSelections.value = selections;
  return true;
};

const handleMediaInfoUndoShortcut = (event) => {
  if (!showMediaInfoEditor.value || event.isComposing) return;
  if (!(event.ctrlKey || event.metaKey) || event.altKey || event.shiftKey) return;
  if (String(event.key || '').toLowerCase() !== 'z') return;

  const target = event.target;
  if (target instanceof Element && target.closest('.media-info-title-input')) return;
  if (!undoLastMediaInfoLanguageChange()) return;

  event.preventDefault();
  event.stopPropagation();
};

const handleLanguageDoubleClick = (stream) => {
  const option = getMediaInfoLanguageOption(stream);
  const title = getMediaInfoLanguageTitle(stream.Type, option?.label, getMediaInfoLanguageSelection(stream));
  if (title) {
    stream.Title = stripChineseScriptParenthetical(title);
  }

  nextTick(() => {
    const instance = mediaInfoLanguageSelectRefs.get(getMediaInfoStreamRowKey(stream));
    if (typeof instance?.blur === 'function') instance.blur();
    const root = instance?.$el || instance;
    root?.querySelector?.('input')?.blur();
  });
};

const fillLanguagesToTitles = () => {
  let filledCount = 0;
  let skippedChineseSubtitleCount = 0;
  let skippedUnknownCount = 0;

  displayStreams.value.forEach((stream) => {
    const option = getMediaInfoLanguageOption(stream);
    const selectedLanguage = getMediaInfoLanguageSelection(stream);
    if (isChineseSubtitleStream(stream, selectedLanguage, option?.label)) {
      skippedChineseSubtitleCount += 1;
      return;
    }
    const title = getMediaInfoLanguageTitle(stream.Type, option?.label, selectedLanguage);
    if (!title) {
      skippedUnknownCount += 1;
      return;
    }
    stream.Title = stripChineseScriptParenthetical(title);
    filledCount += 1;
  });

  if (filledCount > 0) {
    const skippedText = [
      skippedChineseSubtitleCount > 0 ? `，已跳过 ${skippedChineseSubtitleCount} 条中文字幕` : '',
      skippedUnknownCount > 0 ? `，已跳过 ${skippedUnknownCount} 条语言为空或未知的轨道` : '',
    ].join('');
    message.success(`已将语言填入 ${filledCount} 条标题${skippedText}`);
  } else if (skippedChineseSubtitleCount > 0) {
    message.info(`没有可填充的标题，已跳过 ${skippedChineseSubtitleCount} 条中文字幕`);
  } else if (skippedUnknownCount > 0) {
    message.info(`已跳过 ${skippedUnknownCount} 条语言为空或未知的轨道`);
  } else {
    message.warning('没有可填充的语言标签');
  }
};

// 2. 打开编辑器并获取数据 (修改为接收 targetId 参数)
const openMediaInfoEditor = async (targetId) => {
  mediaInfoSaveFeedback.value = null;
  currentEditMediaId.value = String(targetId); // 记录当前正在编辑的 ID

  // 检查其中一个数组是否为空即可
  if (audioLanguageOptions.value.length === 0) {
    await fetchLanguageMapping();
  }

  const loadingMsg = message.loading("正在读取底层媒体指纹...", { duration: 0 });
  try {
    const res = await axios.get(`/api/media_info/edit/${targetId}`);
    mediaInfoContext.value = {
      sha1: res.data.sha1,
      fingerprint_pending: Boolean(res.data.fingerprint_pending),
      media_path: res.data.media_path,
      mediainfo_path: res.data.mediainfo_path
    };
    mediaInfoVersions.value = Array.isArray(res.data.versions) ? res.data.versions : [];
    mediaInfoIsoLanguageOptions.value = Array.isArray(res.data.iso_language_options)
      ? res.data.iso_language_options
      : [];
    subtitleChineseDefaultLanguage.value = res.data.subtitle_chinese_default_language === 'zho'
      ? 'zho'
      : 'chs';
    mediaInfoData.value = res.data.mediainfo;

    let streams = [];
    if (Array.isArray(mediaInfoData.value) && mediaInfoData.value.length > 0) {
      if (mediaInfoData.value[0].MediaSourceInfo) {
        streams = mediaInfoData.value[0].MediaSourceInfo.MediaStreams || [];
      } else {
        streams = mediaInfoData.value[0].MediaStreams || [];
      }
    } else if (mediaInfoData.value && mediaInfoData.value.MediaStreams) {
      streams = mediaInfoData.value.MediaStreams;
    }

    // 过滤出视频、音轨和字幕
    mediaStreams.value = streams.filter(s => s.Type === 'Video' || s.Type === 'Audio' || s.Type === 'Subtitle');
    mediaStreams.value.forEach(ensureMediaInfoLanguageOption);
    mediaInfoLanguageSelections.value = Object.fromEntries(
      mediaStreams.value
        .filter(stream => stream.Type === 'Audio' || stream.Type === 'Subtitle')
        .map(stream => [
          getMediaInfoStreamSelectionKey(stream),
          resolveInitialMediaInfoLanguageSelection(stream),
        ])
    );
    const snapshots = createMediaStreamSnapshots(mediaStreams.value);
    mediaInfoStreamSnapshotMap = new WeakMap(
      mediaStreams.value.map((stream, index) => [stream, snapshots[index]])
    );
    deletedMediaStreamSnapshots.value = [];
    mediaInfoLanguageUndoStack.length = 0;

    showMediaInfoEditor.value = true;
  } catch (e) {
    message.error(e.response?.data?.error || "获取媒体信息失败");
  } finally {
    loadingMsg.destroy();
  }
};

const handleMediaInfoVersionChange = (targetId) => {
  const version = mediaInfoVersions.value.find(item => String(item.emby_id) === String(targetId || ''));
  const firstSegmentId = version?.segments?.[0]?.emby_id;
  const editTargetId = String(firstSegmentId || targetId || '');
  if (editTargetId === String(currentEditMediaId.value || '')) return;
  openMediaInfoEditor(editTargetId);
};

const handleMediaInfoSegmentChange = (targetId) => {
  if (String(targetId || '') === String(currentEditMediaId.value || '')) return;
  openMediaInfoEditor(targetId);
};

const deleteCurrentMediaVersion = async () => {
  const targetId = String(currentMediaInfoVersion.value?.emby_id || '').trim();
  if (!targetId || mediaInfoVersionOptions.value.length <= 1) return;

  isDeletingMediaVersion.value = true;
  const loadingMsg = message.loading('正在删除 115、STRM 和 Emby 当前版本...', { duration: 0 });
  try {
    const response = await axios.post(`/api/media_info/delete_version/${targetId}`);
    const remainingItemId = String(response.data?.remaining_item_id || '').trim();
    showMediaInfoEditor.value = false;
    message.success(response.data?.message || '当前版本已删除并刷新');

    if (remainingItemId) {
      itemId.value = remainingItemId;
      currentEditMediaId.value = remainingItemId;
      renameTargetId.value = remainingItemId;
      await router.replace({ name: 'MediaEditPage', params: { itemId: remainingItemId } });
    }
    await fetchMediaDetails();
  } catch (error) {
    message.error(error.response?.data?.error || '删除当前版本失败，请检查日志');
  } finally {
    loadingMsg.destroy();
    isDeletingMediaVersion.value = false;
  }
};

// 3. 保存修改 (修改为使用 currentEditMediaId)
const saveMediaInfo = async () => {
  isSavingMediaInfo.value = true;
  mediaInfoSaveFeedback.value = null;
  const loadingMsg = message.loading("正在保存媒体信息并通知 Emby 重新加载...", { duration: 0 });

  try {
    const payload = {
      ...mediaInfoContext.value,
      mediainfo: mediaInfoData.value,
    };

    if (applyToSelectedEpisodes.value && selectedEpisodeIds.value.length > 0) {
      payload.target_item_ids = selectedEpisodeIds.value;
      payload.stream_patches = buildStreamPatches();
    }

    // 使用 currentEditMediaId 而不是全局的 itemId
    const res = await axios.post(`/api/media_info/edit/${currentEditMediaId.value}`, payload);
    const reports = Array.isArray(res.data.batch_reports) ? res.data.batch_reports : [];
    const skippedReports = reports.filter(report => Array.isArray(report.skipped) && report.skipped.length > 0);
    if (skippedReports.length > 0) {
      const skippedById = new Map(skippedReports.map(report => [String(report.item_id), report.skipped]));
      episodesList.value = episodesList.value.map((ep) => {
        const skipped = skippedById.get(String(ep.emby_id));
        if (!skipped) return ep;
        const reasons = skipped.map(item => {
          const type = item.type === 'Audio' ? '音轨' : item.type === 'Subtitle' ? '字幕' : '轨道';
          const ordinal = item.ordinal !== undefined && item.ordinal !== null ? ` #${item.ordinal}` : '';
          return `${type}${ordinal}：${item.reason}`;
        });
        return {
          ...ep,
          track_status: 'inconsistent',
          track_difference: `本次批量保存跳过 ${reasons.join('；')}`
        };
      });
      mediaInfoSaveFeedback.value = {
        type: 'warning', message: res.data.message || '部分轨道未更新',
        details: skippedReports.flatMap(report => report.skipped.map(item =>
          `${report.item_id} · ${report.path || ''}：${item.reason}`)),
      };
      message.warning(res.data.message || "部分视频的轨道无法唯一匹配");
    } else {
      message.success(res.data.message || "媒体信息已更新！");
    }
    if (!skippedReports.length) showMediaInfoEditor.value = false;
  } catch (e) {
    const result = e.response?.data || {};
    mediaInfoSaveFeedback.value = {
      type: 'error', message: result.message || result.error || '保存失败',
      details: Array.isArray(result.failures) && result.failures.length ? result.failures : [result.error || '请检查后端日志'],
    };
    message.error(mediaInfoSaveFeedback.value.message);
  } finally {
    loadingMsg.destroy();
    isSavingMediaInfo.value = false;
  }
};

const repairCurrentAudioTracks = async () => {
  const targetId = String(currentEditMediaId.value || '').trim();
  if (!targetId || isRepairingTruehdCore.value) return;

  isRepairingTruehdCore.value = true;
  const loadingMsg = message.loading('正在检查当前媒体音轨，歧义时将只读探测 CD2...', { duration: 0 });
  let repaired = false;
  try {
    const res = await axios.post(`/api/media_info/repair_truehd_core/${targetId}`);
    repaired = Number(res.data?.result?.repaired || 0) > 0;
    if (repaired) {
      if (res.data.warnings) message.warning(res.data.message);
      else message.success(res.data.message || '当前媒体音轨已纠正');
    } else {
      message.info(res.data.message || '当前媒体无需纠正');
    }
  } catch (error) {
    const errorMessage = error.response?.data?.error || '纠正当前媒体音轨失败';
    if (error.response?.status === 409) message.warning(errorMessage);
    else message.error(errorMessage);
  } finally {
    loadingMsg.destroy();
    isRepairingTruehdCore.value = false;
  }

  if (repaired) {
    await openMediaInfoEditor(targetId);
  }
};

// 4. 修正语言与标题独立；只有双击或“语言填入标题”才改写 Title。
const handleLanguageChange = (stream, val) => {
  recordMediaInfoLanguageUndo(stream);
  mediaInfoLanguageSelections.value = {
    ...mediaInfoLanguageSelections.value,
    [getMediaInfoStreamSelectionKey(stream)]: val ?? null,
  };
  updateMediaInfoStreamLanguage(stream, val);
};

// 5. 处理默认勾选变更（同类型单选互斥）
const handleDefaultChange = (changedStream, isChecked) => {
  if (isChecked) {
    mediaStreams.value.forEach(s => {
      if (s !== changedStream && s.Type === changedStream.Type) {
        s.IsDefault = false;
      }
    });
  }
};

// 6. 计算属性：只显示有效音轨和字幕（过滤掉视频流及无效探测流）
const displayStreams = computed(() => {
  return mediaStreams.value.filter(s => (
    s.Type === 'Subtitle'
    || (s.Type === 'Audio' && !isInvalidAudioProbeStream(s))
  ));
});

// 7. 设置硬字幕标记（写入视频流 Title）
const setHardcodedSubtitle = (subtitleType) => {
  // 找到视频流
  const videoStream = mediaStreams.value.find(s => s.Type === 'Video');
  if (videoStream) {
    videoStream.Title = subtitleType;
    message.success(`已设置硬字幕标记：${subtitleType}`);
  } else {
    message.warning('未找到视频流');
  }
};

// 8. 清除硬字幕标记
const clearHardcodedSubtitle = () => {
  const videoStream = mediaStreams.value.find(s => s.Type === 'Video');
  if (videoStream) {
    videoStream.Title = '';
    message.success('已清除硬字幕标记');
  }
};

// ★★★ TMDb 选图相关状态 ★★★
const showTmdbSelector = ref(false);
const isFetchingTmdbImages = ref(false);
const tmdbImagesCache = ref(null); // 缓存拉取到的所有图片
const currentTmdbImages = ref([]); // 当前展示的图片列表
const currentTmdbImageLabel = ref('');
const currentTmdbImageAspect = ref('2/3');

// 打开 TMDb 选图框
const openTmdbSelector = async (type, tmdbKey) => {
  currentEditImageType.value = type;

  // 设置 UI 标题和比例
  const imgConfig = imageTypes.find(i => i.type === type);
  currentTmdbImageLabel.value = imgConfig.label;
  currentTmdbImageAspect.value = imgConfig.aspect;

  showTmdbSelector.value = true;

  // 如果还没拉取过，去后端拉取
  if (!tmdbImagesCache.value) {
    isFetchingTmdbImages.value = true;
    try {
      const res = await axios.get(`/api/tmdb_images/${itemId.value}`);
      tmdbImagesCache.value = res.data;
    } catch (error) {
      message.error("获取 TMDb 图片失败");
      showTmdbSelector.value = false;
      isFetchingTmdbImages.value = false;
      return;
    }
    isFetchingTmdbImages.value = false;
  }

  // 根据类型展示对应的图片列表
  currentTmdbImages.value = tmdbImagesCache.value[tmdbKey] || [];
};

// 选中 TMDb 图片并提交
const selectTmdbImage = async (originalUrl) => {
  showTmdbSelector.value = false; // 关闭选图框

  // 直接复用之前的上传逻辑，把 TMDb 原图直链发给后端
  const payload = {
    image_type: currentEditImageType.value,
    image_url: originalUrl
  };

  await uploadImagePayload(payload);
};

// 格式化语言显示
const formatLang = (langCode) => {
  if (!langCode || langCode === 'null') return '未分级/无文字';
  const langMap = {
    'zh': 'Chinese', 'zh-CN': 'Chinese (Simplified)', 'zh-TW': 'Chinese (Traditional)',
    'en': 'English', 'ja': 'Japanese', 'ko': 'Korean'
  };
  return langMap[langCode] || langCode.toUpperCase();
};

const searchDropdownOptions = computed(() => {
  const options = [];
  if (searchLinks.value.wikipedia) {
    options.push({
      label: 'Google (维基百科)',
      key: 'wikipedia'
    });
  }
  if (searchLinks.value.google) {
    options.push({
      label: 'Google 搜索',
      key: 'google'
    });
  }
  return options;
});

const handleSearchDropdownSelect = (key) => {
  const url = searchLinks.value[key];
  if (url) {
    window.open(url, '_blank');
  }
};

const getActorImageUrl = (actor) => {
  if (actor.imageUrl) {
    return `/api/image_proxy?url=${encodeURIComponent(actor.imageUrl)}`;
  }
  return '';
};

const itemTypeInChinese = computed(() => {
  if (!itemDetails.value || !itemDetails.value.item_type) {
    return '';
  }
  switch (itemDetails.value.item_type) {
    case 'Movie': return '电影';
    case 'Series': return '电视剧';
    default: return itemDetails.value.item_type;
  }
});

const getAvatarColor = (name) => {
  const colors = ['#f56a00', '#7265e6', '#ffbf00', '#00a2ae', '#4caf50', '#2196f3'];
  if (!name || name.length === 0) return colors[0];
  let hash = 0;
  for (let i = 0; i < name.length; i++) {
    hash = name.charCodeAt(i) + ((hash << 5) - hash);
  }
  const index = Math.abs(hash % colors.length);
  return colors[index];
};

const getTMDbImageUrl = (path, size = 'w185') => {
  if (!path) return '';
  return `https://image.tmdb.org/t/p/${size}${path}`;
};

watch(() => itemDetails.value, (newItemDetails) => {
  if (newItemDetails?.current_emby_cast) {
    editableCast.value = newItemDetails.current_emby_cast.map((actor, index) => ({
      ...actor,
      _temp_id: `actor-${Date.now()}-${index}`,
    }));
  } else {
    editableCast.value = [];
  }
}, { deep: true });

const removeActor = (index) => {
  editableCast.value.splice(index, 1);
  message.info("已从编辑列表移除一个演员（尚未保存）。");
};

const searchActors = async () => {
  if (actorSearchQuery.value.length < 1) {
    actorSearchResults.value = [];
    return;
  }
  isSearchingActors.value = true;
  try {
    const response = await axios.get('/api/mappings/config/tmdb_search_persons', {
      params: { q: actorSearchQuery.value }
    });
    actorSearchResults.value = response.data;
  } catch (error) {
    console.error("搜索演员失败:", error);
    message.error("搜索演员时出错，请检查后端日志。");
  } finally {
    isSearchingActors.value = false;
  }
};

const debouncedSearchActors = debounce(searchActors, 300);

const selectActor = (actor) => {
  if (editableCast.value.some(a => a.tmdbId === actor.id)) {
    message.warning(`演员 "${actor.name}" 已经在列表中了。`);
    return;
  }

  const newActor = {
    tmdbId: actor.id,
    name: actor.name,
    role: '',
    imageUrl: getTMDbImageUrl(actor.profile_path),
    emby_person_id: null,
    _temp_id: `new-actor-${Date.now()}`
  };

  editableCast.value.push(newActor);
  message.success(`已添加演员 "${actor.name}"，请为他/她填写角色名。`);

  showAddActorModal.value = false;
  actorSearchQuery.value = '';
  actorSearchResults.value = [];
};

const translateAllFields = async () => {
  try {
    const payload = {
      cast: editableCast.value,
      title: itemDetails.value.item_name,
      year: itemDetails.value.production_year,
      media_type: itemDetails.value.item_type,
    };

    const response = await axios.post('/api/actions/translate_cast_sa', payload);
    const translatedList = response.data;

    editableCast.value = translatedList.map((actor, index) => ({
      ...actor,
      _temp_id: `translated-actor-${Date.now()}-${index}`
    }));

    message.success("智能翻译完成！");

  } catch (error) {
    console.error("一键翻译失败:", error);
    message.error(error.response?.data?.error || "翻译失败，请检查后端日志。");
  } finally {
    isTranslating.value = false;
  }
};

const fetchMediaDetails = async () => {
  isLoading.value = true;
  try {
    const response = await axios.get(`/api/media_for_editing/${itemId.value}`);
    const redirectItemId = String(response.data?.redirect_item_id || '').trim();
    if (redirectItemId && redirectItemId !== String(itemId.value || '')) {
      itemId.value = redirectItemId;
      currentEditMediaId.value = redirectItemId;
      renameTargetId.value = redirectItemId;
      await router.replace({ name: 'MediaEditPage', params: { itemId: redirectItemId } });
    }
    itemDetails.value = response.data;

    if (response.data && response.data.search_links) {
      searchLinks.value = response.data.search_links;
    }

  } catch (error) {
    message.error(error.response?.data?.error || "获取媒体详情失败。");
    itemDetails.value = null;
  } finally {
    isLoading.value = false;
  }
};

onMounted(() => {
  window.addEventListener('keydown', handleMediaInfoUndoShortcut);
  itemId.value = route.params.itemId;

  if (itemId.value) {
    fetchMediaDetails();
  } else {
    message.error("未提供媒体项ID！");
    isLoading.value = false;
  }
});

onBeforeUnmount(() => {
  window.removeEventListener('keydown', handleMediaInfoUndoShortcut);
});

const goBack = () => {
  if (window.history.state?.back) {
    router.back();
    return;
  }
  router.push({ name: 'ReviewList' });
};

const handleSaveChanges = async () => {
  if (!itemDetails.value?.item_id) return;
  isSaving.value = true;
  try {
    await nextTick();

    const castPayload = editableCast.value.map(actor => {
      return {
        tmdbId: actor.tmdbId,
        name: actor.name,
        role: actor.role,
        emby_person_id: actor.emby_person_id
      };
    });

    const payload = {
      cast: castPayload,
      item_name: itemDetails.value.item_name,
    };

    await axios.post(`/api/update_media_cast_sa/${itemDetails.value.item_id}`, payload);

    message.success("修改已保存，Emby将自动刷新。");
    setTimeout(() => {
      goBack();
    }, 1500);

  } catch (error) {
    console.error("保存修改失败:", error);
    message.error(error.response?.data?.error || "保存修改失败，请检查后端日志。");
  } finally {
    isSaving.value = false;
  }
};
</script>

<style scoped>
.media-edit-page {
  padding:0 0 24px;
  transition: all 0.3s;
}
.rename-result-panel {
  margin: 14px 0 4px;
  border: 1px solid var(--n-border-color);
  border-radius: 6px;
  overflow: hidden;
  background: var(--n-color-embedded);
}
.rename-result-title {
  padding: 10px 14px;
  border-bottom: 1px solid var(--n-border-color);
  color: var(--n-text-color);
  font-size: 14px;
  font-weight: 600;
}
.rename-result-row {
  display: grid;
  grid-template-columns: 112px minmax(0, 1fr);
  align-items: stretch;
  border-bottom: 1px solid var(--n-divider-color);
}
.rename-result-row:last-child {
  border-bottom: 0;
}
.rename-result-label {
  display: flex;
  align-items: flex-start;
  box-sizing: border-box;
  min-width: 0;
  padding: 12px 14px;
  color: var(--n-text-color-3);
  font-size: 12px;
  line-height: 1.5;
  background: var(--n-action-color);
}
.rename-result-row > code,
.rename-result-tree {
  display: block;
  box-sizing: border-box;
  min-width: 0;
  padding: 12px 14px;
  color: var(--n-text-color-2);
  font-size: 12px;
  line-height: 1.5;
  word-break: break-word;
  overflow-wrap: anywhere;
}
.rename-result-tree {
  padding-top: 8px;
  padding-bottom: 8px;
}
.rename-result-tree-node {
  display: flex;
  align-items: center;
  gap: 7px;
  min-width: 0;
  min-height: 28px;
  line-height: 1.5;
}
.rename-result-tree-node code {
  overflow-wrap: anywhere;
  word-break: break-word;
}
@media (max-width: 640px) {
  .rename-result-row {
    grid-template-columns: 1fr;
  }
  .rename-result-label {
    padding: 8px 14px 5px;
    background: transparent;
    border-bottom: 1px solid var(--n-divider-color);
  }
  .rename-result-row > code,
  .rename-result-tree {
    padding-top: 9px;
  }
}

.loading-container, .error-container {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  min-height: 200px;
}

.media-poster {
  width: 100%;
  height: auto;
  background-color: var(--n-card-color);
  aspect-ratio: 2 / 3;
}
@container (max-width: 919px) {
  .media-poster { max-height:260px; aspect-ratio:auto; display:flex; justify-content:center; background:var(--ui-surface-muted); }
  .media-poster :deep(img) { max-height:260px; width:auto !important; max-width:100%; }
}

.poster-placeholder, .avatar-placeholder {
  width: 100%;
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  background-color: var(--n-action-color);
}

.actor-grid-container {
  display: grid;
  grid-template-columns:repeat(auto-fit,minmax(min(100%,270px),1fr));
  gap: 16px;
}
.actor-grid-item { min-width:0; }
.actor-grid-item > .n-card { height:100%; }
.actor-edit-card :deep(.n-card-header) { flex-wrap:nowrap; padding:16px 16px 8px !important; }
.actor-edit-card :deep(.n-card-header__main) { min-width:0; }
.actor-edit-card :deep(.n-card-header__extra) { width:auto; flex-shrink:0; }
.actor-edit-card :deep(.n-card__content) { padding:16px !important; }

.actor-edit-card:hover {
  transform: translateY(-4px);
  box-shadow: var(--n-box-shadow-hover) !important;
}

.actor-card-header {
  display: flex;
  min-width:0;
  align-items: center;
  gap: 8px;
}

.actor-name-title {
  font-weight: 600;
  flex-grow: 1;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.actor-card-content {
  display: flex;
  align-items: center;
  gap: 12px;
}

.actor-avatar-image {
  width:72px;
  height:96px;
  border-radius: var(--n-border-radius);
  flex-shrink: 0;
}

.actor-inputs {
  display: flex;
  flex-direction: column;
  gap: 8px;
  flex-grow: 1;
  flex-basis: 0;
  min-width: 0;
}

.compact-form-item {
  margin-bottom: 0 !important;
}

.sticky-actions {
  position: sticky;
  bottom:0;
  left: 0;
  right: 0;
  padding: 16px 24px;
  background-color:var(--modal-solid-bg-color);
  border-top:1px solid var(--ui-border);
  display: flex;
  justify-content: flex-end;
  z-index: 10;
  margin: 24px -24px 0;
}
@media (max-width:540px) {
  .sticky-actions { padding:16px; margin-inline:-16px; }
  .sticky-actions :deep(.n-space) { width:100%; gap:8px !important; }
  .sticky-actions :deep(.n-button) { padding-inline:10px; }
  .actor-card-content { gap:10px; }
}

.drag-handle {
  cursor: grab;
}
.drag-handle:active {
  cursor: grabbing;
}

.sortable-ghost {
  opacity: 0.4;
  background: var(--n-action-color);
  border: 1px dashed var(--n-border-color);
}
.sortable-drag {
  opacity: 1 !important;
  transform: rotate(2deg);
  box-shadow: 0 10px 20px rgba(0,0,0,0.2);
  z-index: 99;
}

/* =========================================================
   ★★★ 分集选择模态框：季集号阅兵式方阵 ★★★
   ========================================================= */
.episode-selector-board {
  max-height: 70vh;
  overflow-y: auto;
  padding-right: 8px;
}

.episode-selector-summary {
  flex: 1;
  min-width: 220px;
  color: var(--n-text-color-3);
  font-size: 13px;
}

.episode-season-switcher {
  display: flex;
  align-items: center;
  gap: 6px;
  max-width: 100%;
  overflow-x: auto;
  padding-bottom: 2px;
}

.episode-season-switcher-label {
  color: var(--n-text-color-2);
  font-size: 13px;
  font-weight: 600;
  white-space: nowrap;
  flex: 0 0 220px;
}

.episode-selector-toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  flex-wrap: wrap;
  gap: 12px;
  margin-bottom: 12px;
}

.episode-season-block {
  margin-bottom: 14px;
  padding: 12px;
  background-color: var(--n-color-embedded);
  border: 1px solid var(--n-border-color);
  border-radius: 8px;
}

.episode-season-block:last-child {
  margin-bottom: 0;
}

.episode-season-header {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 10px;
}

.episode-season-title {
  font-size: 14px;
  font-weight: 700;
  color: var(--n-text-color);
}

.episode-season-count {
  padding: 1px 7px;
  border-radius: 999px;
  font-size: 12px;
  color: var(--n-text-color-3);
  background-color: var(--n-action-color);
}

.episode-parade-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, 42px);
  gap: 8px;
  align-items: center;
}

.episode-chip-shell {
  position: relative;
  width: 42px;
  height: 32px;
}

.episode-chip-checkbox {
  position: absolute;
  top: -7px;
  right: -6px;
  z-index: 2;
  transform: scale(0.72);
  transform-origin: top right;
}

.episode-chip-issue-dot {
  position: absolute;
  top: -5px;
  left: -4px;
  z-index: 2;
  width: 14px;
  height: 14px;
  border-radius: 50%;
  background-color: var(--ui-error-color);
  color: var(--ui-on-error);
  font-size: 10px;
  font-weight: 700;
  line-height: 14px;
  text-align: center;
}

.episode-chip {
  width: 42px;
  height: 30px;
  padding: 0;
  border: 1px solid var(--ui-border);
  border-radius: 5px;
  background-color: var(--ui-surface-muted);
  color: var(--ui-text);
  font-size: 12px;
  font-weight: 700;
  line-height: 1;
  cursor: pointer;
  transition: border-color 0.15s, color 0.15s, background-color 0.15s, transform 0.1s;
}

.episode-chip:hover {
  transform: translateY(-1px);
}

.episode-chip:active {
  transform: translateY(0);
}

.episode-chip-active {
  box-shadow: 0 0 0 2px color-mix(in srgb, var(--accent-color) 16%, transparent);
}

.episode-chip-selected .episode-chip {
  border-color: var(--accent-color);
  box-shadow: 0 0 0 2px color-mix(in srgb, var(--accent-color) 12%, transparent);
}

.episode-chip-multi-version .episode-chip {
  border-color: var(--ui-warning-color);
  background-color: color-mix(in srgb, var(--ui-warning-color) 12%, var(--ui-surface-muted));
  color: var(--ui-warning-text);
  box-shadow: 0 0 0 2px color-mix(in srgb, var(--ui-warning-color) 14%, transparent);
}

.episode-chip-track-issue .episode-chip {
  border-color: var(--ui-error-color);
  color: var(--ui-error-text);
  box-shadow: 0 0 0 2px color-mix(in srgb, var(--ui-error-color) 12%, transparent);
}

.episode-chip-track-unavailable .episode-chip {
  border-color: var(--ui-warning-color);
  color: var(--ui-warning-text);
  box-shadow: 0 0 0 2px color-mix(in srgb, var(--ui-warning-color) 12%, transparent);
}

.episode-chip-track-unavailable .episode-chip-issue-dot {
  background-color: var(--ui-warning-color);
  color: var(--ui-on-warning);
}

.episode-chip-multi-version.episode-chip-track-issue .episode-chip {
  color: var(--ui-warning-text);
}

.episode-chip:is(:hover, .episode-chip-active) {
  border-color: var(--accent-color);
  background-color: var(--ui-primary-solid);
  color: var(--ui-on-primary);
}

.episode-chip-multi-version .episode-chip:is(:hover, .episode-chip-active) {
  border-color: var(--ui-warning-color);
  background-color: var(--ui-warning-color);
  color: var(--ui-on-warning);
}

.media-info-version-select :deep(.n-base-selection) {
  --n-border: 1px solid var(--ui-warning-color) !important;
  --n-border-hover: 1px solid var(--ui-warning-color) !important;
  --n-border-active: 1px solid var(--ui-warning-color) !important;
  --n-box-shadow-active: 0 0 0 2px color-mix(in srgb, var(--ui-warning-color) 18%, transparent) !important;
}

.media-info-version-toolbar {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  align-items: start;
  gap: 12px;
  width: 100%;
}

.media-info-version-controls {
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-width: 0;
}

.media-info-current-version-name {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: 6px;
  width: 100%;
  min-width: 0;
  padding: 7px 10px;
  border: 1px solid var(--n-border-color);
  border-radius: 6px;
  background-color: var(--n-color-embedded);
  color: var(--n-text-color);
  line-height: 1.5;
  overflow-wrap: anywhere;
}

.media-info-version-option-label {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: 6px;
  min-width: 0;
  line-height: 1.5;
  white-space: normal;
  overflow-wrap: anywhere;
}

.media-info-version-accent {
  color: var(--ui-error-text);
  font-weight: 700;
  white-space: nowrap;
}

.media-info-version-filename {
  min-width: 0;
  color: var(--n-text-color);
  overflow-wrap: anywhere;
}

@media (max-width: 640px) {
  .media-info-version-toolbar {
    grid-template-columns: 1fr;
  }
}

.media-info-single-file-name {
  width: 100%;
  min-width: 0;
  padding: 8px 12px;
  border: 1px solid var(--n-border-color);
  border-radius: 6px;
  background-color: var(--n-color-embedded);
  color: var(--n-text-color);
  line-height: 1.5;
  overflow-wrap: anywhere;
}

.media-info-batch-hint {
  margin-top: 6px;
  color: var(--n-text-color-3);
  font-size: 12px;
}

.stream-technical-label {
  margin-top: 4px;
  color: var(--n-text-color-3);
  font-size: 11px;
  line-height: 1.25;
  white-space: normal;
  overflow-wrap: anywhere;
}

.media-info-title-input {
  width: 100%;
  height: 28px;
  box-sizing: border-box;
  padding: 0 10px;
  color: var(--ui-text);
  background-color: var(--ui-input-bg);
  border: 1px solid var(--ui-input-border);
  border-radius: 6px;
  outline: none;
  font: inherit;
  line-height: 1.5;
  caret-color: var(--n-primary-color);
  transition: border-color 0.3s, box-shadow 0.3s;
}

.media-info-title-input:hover,
.media-info-title-input:focus {
  border-color: var(--n-primary-color);
}

.media-info-title-input:focus {
  box-shadow: 0 0 0 2px var(--accent-glow-color);
}

.media-info-title-input::placeholder {
  color: var(--n-text-color-3);
}

.media-info-table-scroll { overflow-x:auto; max-width:100%; }
.media-info-table-scroll :deep(table) { min-width:740px; }

.episode-selector-board::-webkit-scrollbar {
  width: 6px;
}
.episode-selector-board::-webkit-scrollbar-track {
  background: transparent;
}
.episode-selector-board::-webkit-scrollbar-thumb {
  background: var(--n-border-color);
  border-radius: 4px;
}
.episode-selector-board::-webkit-scrollbar-thumb:hover {
  background: var(--n-text-color-3);
}

@media (max-width: 640px) {
  .episode-season-switcher {
    width: 100%;
  }

  .episode-season-block {
    padding: 10px;
  }

  .episode-parade-grid {
    grid-template-columns: repeat(auto-fill, 38px);
    gap: 5px;
  }

  .episode-chip-shell {
    width: 38px;
  }

  .episode-chip {
    width: 38px;
    height: 26px;
    font-size: 11px;
  }
}

/* =========================================================
   ★★★ 图像编辑模态框 (Emby 排版风格) ★★★
   ========================================================= */
.emby-image-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(min(100%, 220px), 1fr));
  gap: 20px;
  padding: 10px 0;
}

.emby-image-card {
  background-color: var(--n-color-embedded);
  border: 1px solid var(--n-border-color);
  border-radius: 8px;
  padding: 16px;
  display: flex;
  flex-direction: column;
  transition: all 0.2s;
}

.emby-image-card:hover {
  box-shadow: var(--n-box-shadow-hover);
  border-color: var(--n-primary-color);
}

.emby-card-image-container {
  width: 100%;
  /* 强制所有卡片的图片区域为 16:9 横版比例 */
  aspect-ratio: 16 / 9;
  background-color: var(--n-action-color);
  border-radius: 4px;
  overflow: hidden;
  position: relative;
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.1);
  display: flex;
  align-items: center;
  justify-content: center;
}

/* 确保 n-image 组件本身撑满容器 */
.full-image {
  width: 100%;
  height: 100%;
  display: block; /* ⚠️ 删掉原来的 display: flex */
}

/* ⚠️ 关键修复：穿透到内部真实的 img 标签，强制它 100% 宽高并 contain */
.full-image :deep(img) {
  width: 100%;
  height: 100%;
  object-fit: contain !important;
}

/* 修复占位符居中问题（因为去掉了 full-image 的 flex） */
.image-placeholder {
  width: 100%;
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  background-color: var(--n-action-color);
  position: absolute;
  top: 0;
  left: 0;
}

.emby-card-footer {
  margin-top: 16px;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
}

.emby-card-title {
  font-size: 15px;
  font-weight: 600;
  color: var(--n-text-color);
}

.emby-card-actions {
  display: flex;
  gap: 16px;
  justify-content: center;
  width: 100%;
  margin-top: 4px;
}

.emby-action-btn {
  color: var(--n-text-color-3) !important;
  transition: color 0.2s, transform 0.1s;
}

.emby-action-btn:hover {
  color: var(--n-primary-color) !important;
  transform: scale(1.1);
}

/* =========================================================
   ★★★ TMDb 搜索模态框 (Emby 排版风格) ★★★
   ========================================================= */
.tmdb-loading-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  min-height: 300px;
}

.emby-tmdb-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
  gap: 24px 16px;
  padding: 10px 0;
  max-height: 65vh;
  overflow-y: auto;
  padding-right: 8px;
}

.emby-tmdb-grid::-webkit-scrollbar {
  width: 6px;
}
.emby-tmdb-grid::-webkit-scrollbar-track {
  background: transparent;
}
.emby-tmdb-grid::-webkit-scrollbar-thumb {
  background: var(--n-border-color);
  border-radius: 4px;
}
.emby-tmdb-grid::-webkit-scrollbar-thumb:hover {
  background: var(--n-text-color-3);
}

.emby-tmdb-card {
  cursor: pointer;
  display: flex;
  flex-direction: column;
  transition: transform 0.2s;
}

.emby-tmdb-card:hover .tmdb-card-image-wrapper {
  transform: scale(1.03);
  box-shadow: 0 0 0 2px var(--n-primary-color);
}

.tmdb-card-image-wrapper {
  width: 100%;
  background-color: var(--n-action-color);
  border-radius: 6px;
  overflow: hidden;
  transition: all 0.2s;
  box-shadow: var(--n-box-shadow);
  display: flex;
  align-items: center;
  justify-content: center;
}

.tmdb-card-info {
  margin-top: 10px;
  text-align: center;
  line-height: 1.5;
}

.tmdb-provider {
  font-size: 13px;
  font-weight: 500;
  color: var(--n-text-color);
}

.tmdb-meta {
  font-size: 12px;
  color: var(--n-text-color-3);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.tmdb-score {
  font-size: 12px;
  color: var(--n-text-color-3);
}
</style>
