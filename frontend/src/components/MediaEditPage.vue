<template>
  <div class="mp-page editor-page">
    <VRow align="center" class="mb-4">
      <VCol cols="auto"><VBtn icon="mdi-arrow-left" variant="text" aria-label="返回列表" @click="goBack" /></VCol>
      <h1 class="sr-only">手动编辑媒体信息</h1>
    </VRow>

    <VAlert v-if="mediaOperationFeedback" :type="mediaOperationFeedback.type" :title="mediaOperationFeedback.message" variant="tonal" class="mb-4">
      <div v-for="(detail, index) in mediaOperationFeedback.details" :key="index" class="text-wrap">{{ detail }}</div>
    </VAlert>
    <VAlert v-if="creditsExtractionFeedback" :type="creditsExtractionFeedback.type" :title="creditsExtractionFeedback.message" variant="tonal" class="mb-4" data-testid="credits-extraction-feedback">
      <div v-for="detail in creditsExtractionFeedback.details" :key="detail" class="text-wrap">{{ detail }}</div>
    </VAlert>

    <VAlert v-if="pageMediaRecoveryRecords.length || mediaRecoveryError" type="warning" :title="pageMediaRecoveryRecords.length ? (isRecoveryCleanupOnly(pageMediaRecoveryRecords) ? '恢复备份待清理' : '媒体信息需要恢复') : '恢复状态未确认'" variant="tonal" class="mb-4" data-testid="media-recovery-alert">
      <p>{{ isRecoveryCleanupOnly(pageMediaRecoveryRecords) ? '媒体信息与原神医配置已通过验收，恢复备份尚未删除。' : '完成恢复验收与备份清理后才能继续保存媒体信息。' }}</p>
      <div v-for="record in pageMediaRecoveryRecords" :key="record.id" class="mt-3">
        <div class="text-wrap">{{ record.reason }}</div><div class="text-break">恢复目录：{{ record.directory }}</div>
        <div v-for="directory in record.conflict_backup_directories || []" :key="directory" class="text-break">当前 JSON 已另行备份并保留：{{ directory }}</div>
        <div v-for="conflict in record.json_conflicts || []" :key="`${conflict.item_id}:${conflict.path}`" class="mt-3">
          <div class="text-break">当前 JSON：{{ conflict.path }}（媒体 ID：{{ conflict.item_id }}）</div>
          <div class="text-break">当前 SHA256：{{ conflict.current_sha256 || '尚未确认' }}</div>
          <VTable density="compact" class="mt-2"><thead><tr><th>变化字段</th><th>原恢复值</th><th>当前值</th></tr></thead><tbody>
            <tr v-for="change in conflict.changes || []" :key="change.path"><td class="text-break">{{ change.path }}</td><td class="text-break">{{ recoveryConflictValue(change.expected) }}</td><td class="text-break">{{ recoveryConflictValue(change.current) }}</td></tr>
          </tbody></VTable>
          <div v-if="conflict.change_count > (conflict.changes?.length || 0)" class="text-caption">另有 {{ conflict.change_count - (conflict.changes?.length || 0) }} 处字段变化</div>
        </div>
        <VBtn class="mt-2" color="warning" variant="flat" :prepend-icon="record.kind === 'verified-cleanup' ? 'mdi-delete-outline' : (record.json_conflicts?.length ? 'mdi-content-save-outline' : 'mdi-restore')" :loading="isRecoveringMediaInfo" :disabled="!record.can_recover || isCheckingMediaRecovery || isConfirmingMediaRecovery || (record.json_conflicts?.length && !recoveryJsonConflictHashes(record))" :data-testid="`media-recovery-${record.id}`" @click="confirmMediaRecovery(record)">{{ record.kind === 'verified-cleanup' ? '清理恢复备份' : (record.json_conflicts?.length ? '备份当前 JSON 并恢复' : '恢复媒体信息') }}</VBtn>
      </div>
      <div v-if="mediaRecoveryError" class="mt-3 text-wrap">{{ mediaRecoveryError }}</div>
      <VBtn class="mt-3" variant="text" :loading="isCheckingMediaRecovery" :disabled="isRecoveringMediaInfo" @click="loadMediaRecovery">重新检查恢复状态</VBtn>
    </VAlert>

    <div v-if="isLoading" class="loading-container">
      <VProgressCircular indeterminate size="48" /><p class="mt-4 text-medium-emphasis">正在加载媒体详情...</p>
    </div>

    <VRow v-else-if="itemDetails && itemDetails.item_name" class="media-edit-page">
      <VCol cols="12" md="3">
        <VCard class="mb-6">
          <VImg :src="posterUrl" :aspect-ratio="2 / 3" cover class="media-poster" :alt="itemDetails.item_name" @click="previewMediaImage(posterUrl, itemDetails.item_name)">
            <template #placeholder><div class="poster-placeholder"><VProgressCircular indeterminate /></div></template>
            <template #error><div class="poster-placeholder"><VIcon icon="mdi-image-outline" size="48" /></div></template>
          </VImg>
          <VCardTitle class="d-flex align-center ga-2 text-wrap">
            <span class="flex-grow-1">{{ itemDetails.item_name }}</span>
            <VChip :color="itemDetails.item_type === 'Movie' ? 'info' : 'success'" size="small">{{ itemTypeInChinese }}</VChip>
          </VCardTitle>
          <VList density="compact">
            <VListItem title="Emby ItemID" :subtitle="String(itemDetails.item_id || '')" />
            <VListItem v-if="itemDetails.review_reason" title="待复核原因" :subtitle="itemDetails.review_reason" class="text-error" />
          </VList>
          <VCardText class="d-flex flex-column ga-3">
            <VBtn block variant="tonal" prepend-icon="mdi-image-multiple-outline" @click="showImageEditor = true">编辑图像</VBtn>
            <VBtn block color="info" variant="tonal" prepend-icon="mdi-file-document-outline" data-testid="media-info-open" @click="handleMediaInfoButtonClick">编辑媒体信息 (音轨/字幕)</VBtn>
            <VBtn block variant="tonal" prepend-icon="mdi-shield-check-outline" :loading="isRevalidatingFingerprints" @click="revalidateFingerprints(itemDetails.item_id || itemId)">重新校验指纹（{{ fingerprintScopeLabel }}）</VBtn>
            <VBtn v-if="['Series', 'Season', 'Episode'].includes(itemDetails.item_type)" block color="warning" variant="tonal" prepend-icon="mdi-image-refresh-outline" :loading="isReplacingEpisodeImages" @click="confirmEpisodeImageReplacement">替换剧集图</VBtn>
            <VBtn v-if="['Series', 'Season', 'Episode'].includes(itemDetails.item_type)" block color="default" variant="tonal" class="media-credits-action" prepend-icon="mdi-movie-open-outline" :loading="isExtractingCredits" :disabled="creditsExtractionBlocked" :data-testid="itemDetails.item_type === 'Episode' ? 'episode-extract-credits' : 'scope-extract-credits'" @click="extractEpisodeCredits(itemDetails.item_id || itemId)">提取{{ fingerprintScopeLabel }}片尾</VBtn>
            <VBtn block color="warning" variant="tonal" prepend-icon="mdi-file-edit-outline" @click="openRenameEditor">重命名媒体</VBtn>
            <VBtn block color="warning" variant="tonal" prepend-icon="mdi-cloud-search-outline" :loading="isRepairingP115Orphan" @click="confirmMediaAction('检查115孤儿文件', `确认只检查${p115OrphanScopeLabel}？`, repairCurrentP115Orphans)">检查115孤儿文件</VBtn>
          </VCardText>
        </VCard>
        <VCard title="辅助工具">
          <VCardText class="d-flex flex-column ga-3">
            <div class="d-flex ga-1">
              <VBtn class="flex-grow-1" variant="tonal" :href="searchLinks.baidu" target="_blank" :disabled="!searchLinks.baidu" :loading="isLoading" prepend-icon="mdi-magnify">百度搜索</VBtn>
              <VMenu>
                <template #activator="{ props }"><VBtn v-bind="props" variant="tonal" icon="mdi-chevron-down" :disabled="searchDropdownOptions.length === 0" aria-label="其他搜索方式" /></template>
                <VList><VListItem v-for="option in searchDropdownOptions" :key="option.key" :title="option.label" @click="handleSearchDropdownSelect(option.key)" /></VList>
              </VMenu>
            </div>
            <VBtn block color="info" variant="tonal" prepend-icon="mdi-translate" :loading="isTranslating" :disabled="isLoading || isSaving" @click="translateAllFields">一键翻译</VBtn>
          </VCardText>
        </VCard>
      </VCol>

      <VCol cols="12" md="9">
        <VCard class="dashboard-card" title="演员列表">
          <VCardText>
            <VForm>
              <draggable v-model="editableCast" tag="div" item-key="_temp_id" class="actor-grid-container" handle=".drag-handle" :animation="300" :disabled="isSaving">
                <template #item="{ element: actor, index }">
                  <VCard class="actor-edit-card" variant="outlined">
                    <VCardTitle class="actor-card-header">
                      <VAvatar :color="getAvatarColor(actor.name)" size="28" :style="{ color: textColorForBackground(getAvatarColor(actor.name)) }">{{ index + 1 }}</VAvatar>
                      <span class="actor-name-title" :title="actor.name">{{ actor.name || '新演员' }}</span>
                      <VBtn icon="mdi-drag" size="small" variant="text" color="default" class="drag-handle" :disabled="isSaving" :aria-label="`拖动演员${actor.name || index + 1}`" />
                      <VBtn icon="mdi-delete-outline" size="small" variant="text" color="error" :disabled="isSaving" :aria-label="`删除演员${actor.name || index + 1}`" @click="confirmMediaAction('删除演员', `确定要删除演员“${actor.name || '新演员'}”吗？`, () => removeActor(index))" />
                    </VCardTitle>
                    <VCardText>
                      <div class="actor-card-content">
                        <VImg :src="getActorImageUrl(actor)" :alt="actor.name || '演员照片'" width="72" height="96" cover class="actor-avatar-image" @click="previewMediaImage(getActorImageUrl(actor), actor.name)">
                          <template #error><div class="avatar-placeholder"><VIcon icon="mdi-account-outline" size="24" /></div></template>
                        </VImg>
                        <div class="actor-inputs">
                          <VTextField v-model="actor.name" label="演员" placeholder="演员名" density="compact" :disabled="isSaving" />
                          <VTextField v-model="actor.role" label="角色" placeholder="角色名" density="compact" :disabled="isSaving" />
                        </div>
                      </div>
                      <VAlert v-if="actor.matchStatus === '翻译未完成' || actor.translationWarning" type="warning" title="翻译未完成" variant="tonal" class="mt-3">{{ actor.translationWarning || '姓名或角色尚未翻译完成，请检查后再保存。' }}</VAlert>
                    </VCardText>
                  </VCard>
                </template>
              </draggable>
            </VForm>
          </VCardText>
          <VCardActions class="editor-actions">
            <VBtn variant="tonal" prepend-icon="mdi-plus" :disabled="isSaving" @click="showAddActorModal = true">添加演员</VBtn>
            <VSpacer /><VBtn variant="text" prepend-icon="mdi-arrow-left" @click="goBack">返回列表</VBtn>
            <VBtn color="primary" variant="flat" prepend-icon="mdi-content-save-outline" :loading="isSaving" :disabled="isExtractingCredits" @click="handleSaveChanges">保存</VBtn>
          </VCardActions>
        </VCard>
      </VCol>
    </VRow>

    <VAlert v-else title="错误" type="error" variant="tonal">无法加载媒体详情，或指定的媒体项不存在。请检查后端日志或确认该媒体项有效。<VBtn variant="text" class="ml-3" @click="goBack">返回列表</VBtn></VAlert>

    <VDialog v-model="showEpisodeSelector" max-width="980" scrollable>
      <VCard>
        <VCardTitle class="d-flex align-center ga-3"><span class="flex-grow-1 text-wrap">{{ itemDetails?.item_type === 'Series' ? '选择季和要编辑的视频' : '选择要编辑的视频' }}</span><VBtn icon="mdi-close" variant="text" color="default" aria-label="关闭分集选择" @click="showEpisodeSelector = false" /></VCardTitle>
        <VProgressLinear v-if="isFetchingEpisodes" indeterminate />
        <VCardText>
          <VBtn variant="tonal" prepend-icon="mdi-shield-check-outline" :loading="isRevalidatingFingerprints" class="mb-3" @click="revalidateFingerprints(itemDetails.item_id || itemId)">重新校验指纹（{{ fingerprintScopeLabel }}）</VBtn>
          <div v-if="episodesList.length > 0" class="episode-selector-board">
            <div class="episode-selector-toolbar">
              <div v-if="itemDetails.item_type === 'Series'" class="episode-season-switcher">
                <span class="episode-season-switcher-label">选择季</span>
                <VBtn v-for="season in episodeSeasonGroups" :key="season.season_number" size="small" :variant="season.season_number === selectedSeasonNumber ? 'flat' : 'tonal'" @click="handleSeasonChange(season.season_number)">{{ formatSeasonLabel(season.season_number) }}</VBtn>
              </div>
              <div class="episode-selector-summary">{{ itemDetails.item_type === 'Series' ? '当前季' : '本季' }} {{ visibleSeasonEpisodes.length }} 个视频，已选 {{ selectedEpisodeIds.length }} 个。</div>
              <div class="d-flex flex-wrap align-center ga-2">
                <VSelect v-if="currentSeasonHasMultipleVersions" v-model="selectedBatchVersionIndex" :items="batchVersionOptions" item-title="label" item-value="value" label="选择版本" density="compact" class="batch-version-select" />
                <VBtn v-if="currentSeasonHasMultipleVersions" size="small" variant="tonal" :disabled="loadingConfig || selectedBatchVersionIndex === null" @click="selectEpisodesByVersion">选择此版本</VBtn>
                <VBtn size="small" variant="tonal" :disabled="loadingConfig" @click="selectAllEpisodes">全选</VBtn><VBtn size="small" variant="tonal" @click="clearSelectedEpisodes">清空</VBtn>
              </div>
            </div>
            <section v-if="selectedSeasonGroup" class="episode-season-block">
              <div class="episode-season-header"><span class="episode-season-title">{{ formatSeasonLabel(selectedSeasonGroup.season_number) }}</span><VChip size="small" variant="tonal">{{ selectedSeasonGroup.episodes.length }} 个视频</VChip></div>
              <div class="episode-parade-grid">
                <div v-for="ep in selectedSeasonGroup.episodes" :key="ep.emby_id" :class="['episode-chip-shell', { 'episode-chip-selected': isEpisodeSelected(ep.emby_id), 'episode-chip-multi-version': Number(ep.version_count || 0) > 1, 'episode-chip-track-issue': ep.track_status === 'inconsistent' }]">
                  <span v-if="ep.track_status === 'inconsistent'" class="episode-chip-issue-dot">!</span>
                  <VCheckboxBtn class="episode-chip-checkbox" :model-value="isEpisodeSelected(ep.emby_id)" :aria-label="`选择${formatEpisodeLabel(ep)}`" @update:model-value="checked => toggleEpisodeSelection(ep.emby_id, checked)" />
                  <VBtn size="small" class="episode-chip" :variant="String(currentEditMediaId || '') === String(ep.emby_id || '') ? 'flat' : 'tonal'" :color="ep.track_status === 'inconsistent' ? 'error' : Number(ep.version_count || 0) > 1 ? 'warning' : 'primary'" :title="getEpisodeTooltip(ep)" @click="handleEpisodeChipClick(ep)">{{ formatEpisodeLabel(ep) }}</VBtn>
                </div>
              </div>
            </section>
          </div>
          <VEmptyState v-else-if="!isFetchingEpisodes" title="未找到在库的分集" text="请确认该剧集已入库" icon="mdi-television-off" />
        </VCardText>
        <VCardActions><VSpacer /><VBtn variant="text" @click="showEpisodeSelector = false">关闭</VBtn></VCardActions>
      </VCard>
    </VDialog>

    <VDialog v-model="showAddActorModal" max-width="600" scrollable>
      <VCard>
        <VCardTitle class="d-flex align-center"><span class="flex-grow-1">搜索并添加演员</span><VBtn icon="mdi-close" variant="text" color="default" aria-label="关闭演员搜索" @click="showAddActorModal = false" /></VCardTitle>
        <VCardText>
          <VTextField v-model="actorSearchQuery" label="演员名" placeholder="输入演员名进行搜索..." clearable prepend-inner-icon="mdi-magnify" @update:model-value="value => { actorSearchQuery = value || ''; debouncedSearchActors(); }" />
          <VProgressLinear v-if="isSearchingActors" indeterminate class="mt-3" />
          <VList class="mt-3">
            <VListItem v-for="actor in actorSearchResults" :key="actor.id" :title="actor.name" :subtitle="actor.known_for ? `代表作: ${actor.known_for}` : actor.department || '表演'" @click="selectActor(actor)">
              <template #prepend><VAvatar :image="getTMDbImageUrl(actor.profile_path, 'w92')" size="48"><VIcon v-if="!actor.profile_path" icon="mdi-account-outline" /></VAvatar></template>
              <template #append><VBtn size="small" variant="tonal" @click.stop="selectActor(actor)">选择</VBtn></template>
            </VListItem>
          </VList>
          <VEmptyState v-if="!isSearchingActors && actorSearchResults.length === 0 && actorSearchQuery" title="未找到相关人物" icon="mdi-account-search-outline" />
        </VCardText>
      </VCard>
    </VDialog>

    <VDialog v-model="showImageEditor" max-width="1100" scrollable>
      <VCard>
        <VCardTitle class="d-flex align-center"><span class="flex-grow-1">编辑图像</span><VBtn icon="mdi-close" variant="text" color="default" aria-label="关闭图像编辑" @click="showImageEditor = false" /></VCardTitle>
        <VCardText>
          <div class="media-image-grid">
            <VCard v-for="img in imageTypes" :key="img.type" variant="outlined" class="media-image-card">
              <VImg :src="getDynamicImageUrl(img.embyType, img.type)" :aspect-ratio="img.type === 'poster' ? 2 / 3 : 16 / 9" :alt="img.label" class="media-image" @click="previewMediaImage(getDynamicImageUrl(img.embyType, img.type), img.label)">
                <template #error><div class="image-placeholder"><VIcon icon="mdi-image-outline" size="40" /></div></template>
              </VImg>
              <VCardTitle class="text-subtitle-1">{{ img.label }}</VCardTitle>
              <VCardActions class="justify-center">
                <VTooltip text="搜索新图像" location="bottom"><template #activator="{ props }"><VBtn v-bind="props" icon="mdi-magnify" variant="text" :aria-label="`搜索${img.label}`" @click="openTmdbSelector(img.type, img.tmdbKey)" /></template></VTooltip>
                <VTooltip text="上传图像文件" location="bottom"><template #activator="{ props }"><VBtn v-bind="props" icon="mdi-cloud-upload-outline" variant="text" :aria-label="`上传${img.label}`" @click="triggerFileUpload(img.type)" /></template></VTooltip>
                <VTooltip text="设置网络图像" location="bottom"><template #activator="{ props }"><VBtn v-bind="props" icon="mdi-link-variant" variant="text" :aria-label="`设置${img.label}网络图片`" @click="openUrlPrompt(img.type)" /></template></VTooltip>
              </VCardActions>
            </VCard>
          </div>
          <input ref="fileInputRef" type="file" class="d-none" accept="image/jpeg,image/png,image/webp" @change="handleFileUpload" />
        </VCardText>
      </VCard>
    </VDialog>

    <VDialog v-model="showTmdbSelector" max-width="1200" scrollable>
      <VCard>
        <VCardTitle class="d-flex align-center"><span class="flex-grow-1 text-wrap">搜索图像 - {{ currentTmdbImageLabel }}</span><VBtn icon="mdi-close" variant="text" color="default" aria-label="关闭图像搜索" @click="showTmdbSelector = false" /></VCardTitle>
        <VCardText>
          <div v-if="isFetchingTmdbImages" class="tmdb-loading-state"><VProgressCircular indeterminate size="48" /><p class="mt-4 text-medium-emphasis">正在从 TheMovieDb 拉取数据...</p></div>
          <VEmptyState v-else-if="currentTmdbImages.length === 0" title="未找到相关图像" icon="mdi-image-search-outline" />
          <div v-else class="media-tmdb-grid">
            <VCard v-for="(img, index) in currentTmdbImages" :key="index" class="media-tmdb-card" variant="outlined" @click="selectTmdbImage(img.original)">
              <VImg :src="img.preview" :aspect-ratio="currentTmdbImageAspect === '2/3' ? 2 / 3 : 16 / 9" :alt="`${currentTmdbImageLabel}备选图${index + 1}`" />
              <VCardText v-if="img.width && img.height" class="text-center text-caption">{{ img.width }} × {{ img.height }}</VCardText>
            </VCard>
          </div>
        </VCardText>
      </VCard>
    </VDialog>

    <VDialog v-model="showUrlPrompt" max-width="560">
      <VCard title="设置网络图片">
        <VCardText><VTextField v-model="imageUrlInput" label="图片直链" placeholder="请粘贴以 http/https 开头的图片直链..." /></VCardText>
        <VCardActions><VSpacer /><VBtn variant="text" @click="showUrlPrompt = false">取消</VBtn><VBtn variant="flat" :loading="isUploadingImage" @click="submitUrlImage">确定</VBtn></VCardActions>
      </VCard>
    </VDialog>

    <VDialog :model-value="showMediaInfoEditor" :eager="mediaInfoEditorBooted" :retain-focus="!isConfirmingMediaRecovery" max-width="1100" scrollable @update:model-value="value => { if (!value) closeMediaInfoEditor(); }">
      <VCard>
        <VCardTitle class="d-flex align-center"><span class="flex-grow-1">编辑媒体信息 (音轨/字幕)</span><VBtn icon="mdi-close" variant="text" color="default" aria-label="关闭媒体信息编辑器" @click="closeMediaInfoEditor" /></VCardTitle>
        <VCardText>
          <VAlert v-if="editorMediaRecoveryRecords.length || mediaRecoveryError" type="warning" :title="editorMediaRecoveryRecords.length ? (isRecoveryCleanupOnly(editorMediaRecoveryRecords) ? '恢复备份待清理' : '媒体信息需要恢复') : '恢复状态未确认'" variant="tonal" class="mb-4" data-testid="media-recovery-alert">
            <p>{{ isRecoveryCleanupOnly(editorMediaRecoveryRecords) ? '媒体信息与原神医配置已通过验收，恢复备份尚未删除。' : '完成恢复验收与备份清理后才能继续保存媒体信息。' }}</p>
            <div v-for="record in editorMediaRecoveryRecords" :key="record.id" class="mt-3">
              <div class="text-wrap">{{ record.reason }}</div><div class="text-break">恢复目录：{{ record.directory }}</div>
              <div v-for="directory in record.conflict_backup_directories || []" :key="directory" class="text-break">当前 JSON 已另行备份并保留：{{ directory }}</div>
              <div v-for="conflict in record.json_conflicts || []" :key="`${conflict.item_id}:${conflict.path}`" class="mt-3">
                <div class="text-break">当前 JSON：{{ conflict.path }}（媒体 ID：{{ conflict.item_id }}）</div>
                <div class="text-break">当前 SHA256：{{ conflict.current_sha256 || '尚未确认' }}</div>
                <VTable density="compact" class="mt-2"><thead><tr><th>变化字段</th><th>原恢复值</th><th>当前值</th></tr></thead><tbody>
                  <tr v-for="change in conflict.changes || []" :key="change.path"><td class="text-break">{{ change.path }}</td><td class="text-break">{{ recoveryConflictValue(change.expected) }}</td><td class="text-break">{{ recoveryConflictValue(change.current) }}</td></tr>
                </tbody></VTable>
                <div v-if="conflict.change_count > (conflict.changes?.length || 0)" class="text-caption">另有 {{ conflict.change_count - (conflict.changes?.length || 0) }} 处字段变化</div>
              </div>
              <VBtn class="mt-2" color="warning" variant="flat" :prepend-icon="record.kind === 'verified-cleanup' ? 'mdi-delete-outline' : (record.json_conflicts?.length ? 'mdi-content-save-outline' : 'mdi-restore')" :loading="isRecoveringMediaInfo" :disabled="!record.can_recover || isCheckingMediaRecovery || isConfirmingMediaRecovery || (record.json_conflicts?.length && !recoveryJsonConflictHashes(record))" :data-testid="`media-recovery-${record.id}`" @click="confirmMediaRecovery(record)">{{ record.kind === 'verified-cleanup' ? '清理恢复备份' : (record.json_conflicts?.length ? '备份当前 JSON 并恢复' : '恢复媒体信息') }}</VBtn>
            </div>
            <div v-if="mediaRecoveryError" class="mt-3 text-wrap">{{ mediaRecoveryError }}</div>
            <VBtn class="mt-3" variant="text" :loading="isCheckingMediaRecovery" :disabled="isRecoveringMediaInfo" @click="loadMediaRecovery">重新检查恢复状态</VBtn>
          </VAlert>
          <VBtn variant="tonal" prepend-icon="mdi-shield-check-outline" :loading="isRevalidatingFingerprints" class="mb-4" :disabled="isSavingMediaInfo || isDeletingMediaVersion || isRepairingTruehdCore" @click="revalidateFingerprints(currentEditMediaId)">重新校验当前视频指纹</VBtn>
          <VBtn v-if="['Series', 'Season', 'Episode'].includes(itemDetails?.item_type)" color="default" variant="tonal" prepend-icon="mdi-movie-open-outline" class="mb-4 ms-2 media-credits-action" :loading="isExtractingCredits" :disabled="creditsExtractionBlocked" data-testid="media-info-extract-credits" @click="extractEpisodeCredits(currentEditMediaId)">提取当前集片尾</VBtn>
          <VAlert v-if="creditsExtractionFeedback?.itemId === String(currentEditMediaId)" :type="creditsExtractionFeedback.type" :title="creditsExtractionFeedback.message" variant="tonal" class="mb-4" data-testid="media-info-credits-feedback">
            <div v-for="detail in creditsExtractionFeedback.details" :key="detail" class="text-wrap">{{ detail }}</div>
          </VAlert>
          <VAlert v-if="mediaInfoContext.fingerprint_pending" type="warning" variant="tonal" class="mb-4">文件身份待校验。请先重新校验指纹，完成后重新打开编辑器，再保存媒体信息。</VAlert>
          <VAlert v-if="mediaInfoSaveFeedback" :type="mediaInfoSaveFeedback.type" :title="mediaInfoSaveFeedback.message" variant="tonal" class="mb-4">
            <div class="media-info-feedback"><div v-for="(detail, index) in mediaInfoSaveFeedback.details" :key="index">{{ detail }}</div></div>
          </VAlert>
          <p class="text-body-2 text-medium-emphasis mb-4">保存期间可在顶栏查看进度；完成后保留当前编辑器和已保存内容。未通过验收时会显示具体原因。</p>
          <div v-if="mediaInfoVersionOptions.length === 1 && mediaInfoSingleVersionFileName" class="mb-4"><label class="text-caption text-medium-emphasis">媒体文件</label><div class="media-info-single-file-name" :title="mediaInfoSingleVersionFileName">{{ mediaInfoSingleVersionFileName }}</div></div>
          <div v-if="mediaInfoVersionOptions.length > 1" class="media-info-version-toolbar mb-4">
            <div class="media-info-version-controls">
              <div v-if="mediaInfoCurrentVersionFileName" class="media-info-current-version-name" :title="mediaInfoCurrentVersionFileName"><span v-if="mediaInfoCurrentVersionMeta" class="media-info-version-accent">{{ mediaInfoCurrentVersionMeta }}</span><span class="media-info-version-filename">{{ mediaInfoCurrentVersionFileName }}</span></div>
              <VSelect :model-value="currentMediaInfoVersionId" :items="mediaInfoVersionOptions" item-title="label" item-value="value" label="媒体版本" class="media-info-version-select" color="warning" :disabled="isSavingMediaInfo || isRepairingTruehdCore || isDeletingMediaVersion" @update:model-value="handleMediaInfoVersionChange">
                <template #selection="{ item }"><div class="media-info-version-option-label"><span class="media-info-version-accent">{{ mediaInfoVersionMeta(item.raw) }}</span><span class="media-info-version-filename">{{ item.raw.file_name || item.raw.display_name || item.raw.label }}</span></div></template>
                <template #item="{ props, item }"><VListItem v-bind="props" role="option"><template #title><div class="media-info-version-option-label"><span class="media-info-version-accent">{{ mediaInfoVersionMeta(item.raw) }}</span><span class="media-info-version-filename">{{ item.raw.file_name || item.raw.display_name || item.raw.label }}</span></div></template></VListItem></template>
              </VSelect>
            </div>
            <VBtn color="error" variant="tonal" prepend-icon="mdi-delete-outline" :loading="isDeletingMediaVersion" :disabled="isSavingMediaInfo || isRepairingTruehdCore" @click="confirmMediaAction('删除当前版本', '将永久删除当前选中的整套媒体版本，包括115云端文件、本地STRM、NFO、图片、媒体信息和Emby版本。其他格式版本不受影响，确认继续吗？', deleteCurrentMediaVersion)">删除当前版本</VBtn>
          </div>
          <VSelect v-if="mediaInfoSegmentOptions.length > 1" :model-value="String(currentEditMediaId || '')" :items="mediaInfoSegmentOptions" item-title="label" item-value="value" label="电影分段" class="mb-4" :disabled="isSavingMediaInfo || isRepairingTruehdCore" @update:model-value="handleMediaInfoSegmentChange" />
          <VAlert type="info" variant="tonal" class="mb-4">修改语言后保存，系统会重载媒体信息并回读验收。<br /><strong>中文提示：</strong>选择“简体”或“繁体”并保存，Emby 会显示对应字形，原标题独立保留。使用“中文”时，也可通过标题中的简／繁标记识别字形。</VAlert>
          <VAlert v-if="canBatchApplyMediaInfo" type="warning" variant="tonal" class="mb-4">
            <VCheckbox v-model="applyToSelectedEpisodes" :label="`同时应用到已选 ${selectedEpisodeIds.length} 集（当前正在编辑的集也会保存）`" :disabled="isSavingMediaInfo || isRepairingTruehdCore" />
            <p class="text-caption">仅修改可唯一匹配的轨道；无法匹配的音轨和字幕保持原值。</p>
          </VAlert>
          <VCard variant="outlined" class="mb-4" title="标记硬字幕 (写入视频流 Title)">
            <VCardText class="d-flex flex-wrap ga-2">
              <VBtn variant="tonal" :disabled="isSavingMediaInfo || isRepairingTruehdCore" @click="setHardcodedSubtitle('简中硬字幕')">简中</VBtn><VBtn variant="tonal" :disabled="isSavingMediaInfo || isRepairingTruehdCore" @click="setHardcodedSubtitle('繁中硬字幕')">繁中</VBtn>
              <VBtn variant="tonal" :disabled="isSavingMediaInfo || isRepairingTruehdCore" @click="setHardcodedSubtitle('简英硬字幕')">简英</VBtn><VBtn variant="tonal" :disabled="isSavingMediaInfo || isRepairingTruehdCore" @click="setHardcodedSubtitle('繁英硬字幕')">繁英</VBtn>
              <VBtn variant="tonal" :disabled="isSavingMediaInfo || isRepairingTruehdCore" prepend-icon="mdi-close" @click="clearHardcodedSubtitle">清除标记</VBtn>
            </VCardText>
          </VCard>
          <div class="d-flex justify-end mb-3"><VBtn variant="tonal" prepend-icon="mdi-translate" :loading="isFillingMediaInfoTitles" :disabled="isSavingMediaInfo || isRepairingTruehdCore || isDeletingMediaVersion" @click="fillLanguagesToTitles">语言填入标题</VBtn></div>
          <div class="media-info-table-scroll" role="region" aria-label="音轨和字幕编辑表格" tabindex="0">
            <VTable density="compact">
              <thead><tr><th style="width:120px">轨道</th><th style="width:110px">比特率</th><th style="min-width:200px">当前标题 (Title)</th><th style="width:250px">修正语言 (Language)</th><th style="width:80px" class="text-center">默认</th><th style="width:64px" class="text-center">操作</th></tr></thead>
              <tbody>
                <tr v-for="stream in displayStreams" :key="getMediaInfoStreamRowKey(stream)">
                  <td><VChip :color="stream.Type === 'Audio' ? 'info' : 'success'" size="small">{{ stream.Type === 'Audio' ? '音轨' : '字幕' }} {{ getStreamTypeOrdinal(stream) }}</VChip><div class="stream-technical-label">{{ getStreamTechnicalLabel(stream) }}</div></td>
                  <td>{{ stream.Type === 'Audio' ? formatAudioBitRate(stream) : '-' }}</td>
                  <td><VTextField v-model="stream.Title" density="compact" placeholder="无标题" :aria-label="`${getMediaInfoStreamDeleteLabel(stream)}当前标题`" :disabled="isSavingMediaInfo || isRepairingTruehdCore" autocomplete="off" spellcheck="false" :mobile-layout="false" /></td>
                  <td><div title="双击将当前语言填入标题" @dblclick.stop.prevent="handleLanguageDoubleClick(stream)"><VAutocomplete :ref="instance => setMediaInfoLanguageSelectRef(getMediaInfoStreamRowKey(stream), instance)" :model-value="getMediaInfoLanguageSelection(stream)" :search="showMediaInfoEditor ? undefined : ''" :items="stream.Type === 'Audio' ? audioLanguageOptions : subtitleLanguageOptions" item-title="label" item-value="value" :custom-filter="mediaInfoLanguageFilter" placeholder="输入语言名称或代码" clearable density="compact" :mobile-layout="false" :aria-label="`${getMediaInfoStreamDeleteLabel(stream)}修正语言`" :disabled="isSavingMediaInfo || isRepairingTruehdCore || isFillingMediaInfoTitles" @update:model-value="value => handleLanguageChange(stream, value)" /></div></td>
                  <td class="text-center"><VCheckboxBtn v-model="stream.IsDefault" :aria-label="`${getMediaInfoStreamDeleteLabel(stream)}默认轨道`" :disabled="isSavingMediaInfo || isRepairingTruehdCore" @update:model-value="value => handleDefaultChange(stream, value)" /></td>
                  <td class="text-center"><VBtn icon="mdi-delete-outline" variant="text" color="error" size="small" :disabled="isSavingMediaInfo || isRepairingTruehdCore" :aria-label="`删除${getMediaInfoStreamDeleteLabel(stream)}`" @click="confirmMediaAction('删除轨道', `确认从当前编辑草稿中删除“${getMediaInfoStreamDeleteLabel(stream)}”吗？只有保存后才会生效。`, () => removeMediaInfoStream(stream))" /></td>
                </tr>
                <tr v-if="displayStreams.length === 0"><td colspan="6" class="text-center py-6 text-medium-emphasis">未解析到媒体流</td></tr>
              </tbody>
            </VTable>
          </div>
        </VCardText>
        <VCardActions class="editor-actions">
          <VBtn color="warning" variant="tonal" prepend-icon="mdi-wrench-outline" :loading="isRepairingTruehdCore" :disabled="isSavingMediaInfo" @click="confirmMediaAction('纠正多出音轨', '将丢弃当前未保存修改；存在歧义时会只读探测 CD2，并且只纠正证据唯一的高清音轨兼容核心。是否继续？', repairCurrentAudioTracks)">纠正多出音轨</VBtn>
          <VSpacer /><VBtn variant="text" @click="closeMediaInfoEditor">取消</VBtn><VBtn color="primary" variant="flat" prepend-icon="mdi-content-save-outline" :disabled="isRepairingTruehdCore || isFillingMediaInfoTitles || mediaRecoveryBlocked || isExtractingCredits" :loading="isSavingMediaInfo" data-testid="media-info-save" @click="saveMediaInfo">保存</VBtn>
        </VCardActions>
      </VCard>
    </VDialog>

    <VDialog v-model="showRenameEditor" max-width="760" scrollable>
      <VCard>
        <VCardTitle class="d-flex align-center"><span class="flex-grow-1">重命名媒体</span><VBtn icon="mdi-close" variant="text" color="default" aria-label="关闭重命名" @click="showRenameEditor = false" /></VCardTitle>
        <VCardText>
          <VAlert type="info" variant="tonal" class="mb-4">输入新的文件名即可。目录按高级设置中的 MoviePilot 重命名格式生成；源文件扩展名固定，115 文件、STRM 和媒体信息路径会同步更新。</VAlert>
          <div class="d-flex ga-2 mb-4"><VTextField :model-value="renameCustomName" label="新文件名" placeholder="输入文件名" spellcheck="false" @update:model-value="handleRenameNameChange" /><VTextField :model-value="renameExtension" label="扩展名" readonly style="max-width:100px" /></div>
          <VTextField v-model="renamePart" label="指定 Part" placeholder="留空表示未指定，例如：花絮、Part 1" spellcheck="false" class="mb-4" @update:model-value="renamePreview = null" />
          <VSelect v-if="renameVersions.length > 1" v-model="renameTargetId" :items="renameVersionOptions" item-title="label" item-value="value" label="媒体版本" class="mb-4" @update:model-value="switchRenameVersion" />
          <VBtn variant="tonal" prepend-icon="mdi-wrench-outline" :loading="isProbingRename" class="mb-3" @click="probeRenameFields">ffprobe 命名补充并预览</VBtn>
          <p v-if="Object.keys(renameSupplementFields).length" class="text-caption text-medium-emphasis mb-3">ffprobe 补充：{{ Object.entries(renameSupplementFields).map(([key, value]) => `${key}=${value}`).join(' · ') }}</p>
          <div v-if="renamePreview" class="rename-result-panel">
            <div class="rename-result-title">预览结果</div>
            <div class="rename-result-row"><span class="rename-result-label">目标文件</span><code>{{ renamePreview.target_name }}</code></div>
            <div v-if="renamePreview.target_relative_path" class="rename-result-row"><span class="rename-result-label">目标相对路径</span><div class="rename-result-tree"><div v-for="(part, index) in renamePreviewPathParts" :key="`${part}-${index}`" class="rename-result-tree-node" :style="{ paddingLeft: `${index * 18}px` }"><VIcon :icon="index === renamePreviewPathParts.length - 1 ? 'mdi-file-outline' : 'mdi-folder-outline'" :color="index === renamePreviewPathParts.length - 1 ? 'info' : 'warning'" /><code>{{ part }}</code></div></div></div>
            <div v-if="renamePreview.target_cloud_path" class="rename-result-row"><span class="rename-result-label">115 路径</span><code>{{ renamePreview.target_cloud_path }}</code></div>
            <div v-if="renamePreview.target_local_path" class="rename-result-row"><span class="rename-result-label">本地路径</span><code>{{ renamePreview.target_local_path }}</code></div>
          </div>
        </VCardText>
        <VCardActions><VSpacer /><VBtn variant="tonal" prepend-icon="mdi-eye-outline" :loading="isPreviewingRename" @click="previewRename">预览</VBtn><VBtn color="primary" variant="flat" prepend-icon="mdi-content-save-outline" :loading="isExecutingRename" :disabled="!renamePreview" @click="executeRename">保存</VBtn></VCardActions>
      </VCard>
    </VDialog>

    <VDialog :model-value="Boolean(imagePreview)" max-width="1200" @update:model-value="value => { if (!value) imagePreview = null; }">
      <VCard v-if="imagePreview"><VCardTitle class="d-flex align-center"><span class="flex-grow-1 text-wrap">{{ imagePreview.title || '图像预览' }}</span><VBtn icon="mdi-close" variant="text" color="default" aria-label="关闭图像预览" @click="imagePreview = null" /></VCardTitle><VCardText><VImg :src="imagePreview.url" max-height="80vh" /></VCardText></VCard>
    </VDialog>
  </div>
</template>

<script setup>
import { ref, shallowRef, onMounted, onBeforeUnmount, watch, computed, nextTick, toRaw } from 'vue';
import draggable from 'vuedraggable';
import { useRoute, useRouter } from 'vue-router';
import axios from 'axios';
import { useMessage, useDialog } from '@/composables/useFeedback';
import { debounce } from 'lodash-es';
import { textColorForBackground } from '../theme.js';
import { useTheme } from 'vuetify';
import { useTaskStatus } from '../composables/useTaskStatus.js';
import { useConfig } from '../composables/useConfig.js';
import {
  buildMediaInfoLanguageOptions,
  filterMediaInfoLanguageOption,
  findMediaInfoLanguageOption,
  getMediaInfoLanguageAliases,
  getMediaInfoLanguageTitle,
  getMediaInfoLanguageTokens,
  normalizeMediaInfoCodeToken,
  resolveMediaInfoLanguageSelection,
  isInvalidAudioProbeStream,
  updateMediaInfoStreamLanguage,
} from '../utils/mediaStreams.js';

const route = useRoute();
const router = useRouter();
const message = useMessage();
const dialog = useDialog();
const theme = useTheme();
const { isBackgroundTaskRunning } = useTaskStatus();
const { configModel, loadingConfig } = useConfig();
const imagePreview = ref(null);
const previewMediaImage = (url, title) => {
  if (url) imagePreview.value = { url, title };
};
const confirmMediaAction = (title, content, action) => dialog.warning({
  title, content, positiveText: '确认', negativeText: '取消', onPositiveClick: action,
});
const confirmEpisodeImageReplacement = () => {
  const scope = itemDetails.value?.item_type === 'Episode' ? '单集'
    : itemDetails.value?.item_type === 'Season' ? '季所有分集' : '电视剧所有分集';
  confirmMediaAction('替换剧集图', `将删除当前${scope}的 Primary 海报，随后让神医助手使用 ffprobe 截图生成集图。是否继续？`, replaceEpisodeImages);
};
const mediaInfoLanguageFilter = (value, query, item) => filterMediaInfoLanguageOption(query, item?.raw || item);

const itemId = ref(null);
const isLoading = ref(true);
const itemDetails = ref(null);
let mediaDetailsLoadSequence = 0;
const editableCast = ref([]);
const isSaving = ref(false);
let actorSaveSession = 0;
let actorSaveNavigationTimer = null;
const invalidateActorSaveSession = () => {
  ++actorSaveSession;
  if (actorSaveNavigationTimer !== null) clearTimeout(actorSaveNavigationTimer);
  actorSaveNavigationTimer = null;
};

const searchLinks = ref({});
const isTranslating = ref(false);
const mediaOperationFeedback = ref(null);

const showAddActorModal = ref(false);
const actorSearchQuery = ref('');
const actorSearchResults = ref([]);
const isSearchingActors = ref(false);

// ★★★ 图像编辑相关状态 ★★★
const showImageEditor = ref(false);
const showUrlPrompt = ref(false);
let imageUrlPromptSession = 0;
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
  ++imageUrlPromptSession;
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
  const promptSession = imageUrlPromptSession;
  const pageSession = mediaRecoverySession;
  const targetId = String(itemId.value || '');
  const uploaded = await uploadImagePayload(payload);
  if (uploaded && promptSession === imageUrlPromptSession && pageSession === mediaRecoverySession
    && targetId === String(itemId.value || '') && payload.image_type === currentEditImageType.value
    && payload.image_url === imageUrlInput.value) showUrlPrompt.value = false;
};

const uploadImagePayload = async (payload) => {
  const targetId = String(itemId.value || '').trim();
  if (!targetId || isUploadingImage.value) return false;
  const imageType = payload instanceof FormData ? payload.get('image_type') : payload.image_type;
  const sequence = mediaDetailsLoadSequence;
  const session = mediaRecoverySession;
  const isCurrentItem = () => sequence === mediaDetailsLoadSequence
    && session === mediaRecoverySession && targetId === String(itemId.value || '');
  isUploadingImage.value = true;
  const loadingMsg = message.loading("正在替换并通知 Emby 刷新...", { duration: 0 });

  try {
    const config = payload instanceof FormData
      ? { headers: { 'Content-Type': 'multipart/form-data' } }
      : {};

    const res = await axios.post(`/api/update_media_image/${targetId}`, payload, config);
    if (!isCurrentItem()) return false;
    message.success(res.data.message || "图片替换成功！");

    // 刷新对应图片的时间戳，强制前端重新加载图片
    imageRefreshTokens.value[imageType] = Date.now();
    return true;

  } catch (error) {
    if (!isCurrentItem()) return false;
    console.error("图片替换失败:", error);
    message.error(error.response?.data?.error || "图片替换失败，请检查后端日志。");
    return false;
  } finally {
    loadingMsg.destroy();
    isUploadingImage.value = false;
  }
};

const mediaOperationDetails = (values) => {
  const details = [...new Set(values.map(value => {
    const text = typeof value === 'string' ? value : value && typeof value === 'object'
      ? [value.item_id || value.path, value.reason || value.error || value.message || '处理未完成'].filter(Boolean).join('：') : '';
    return text.replace(/^媒体信息 JSON 已保存，Emby 刷新或回读(?:尚)?未确认[：:]\s*/, '').replace(/^[：:\s]+/, '').trim();
  }).filter(Boolean))];
  return details.filter(detail => !details.some(other => other !== detail && other.endsWith(`：${detail}`)));
};

const replaceEpisodeImages = async () => {
  const targetId = String(itemId.value || '').trim();
  if (!targetId || isReplacingEpisodeImages.value) return;
  const sequence = mediaDetailsLoadSequence;
  const session = mediaRecoverySession;
  const isCurrentItem = () => sequence === mediaDetailsLoadSequence
    && session === mediaRecoverySession && targetId === String(itemId.value || '');
  isReplacingEpisodeImages.value = true;
  mediaOperationFeedback.value = null;
  const loadingMsg = message.loading('剧集图正在后台处理，可在通知中心中查看生成数量...', { duration: 0 });
  try {
    const response = await axios.post(`/api/media_info/replace_episode_images/${targetId}`);
    if (!isCurrentItem()) return;
    const result = response.data || {};
    const details = mediaOperationDetails([
      ...(Array.isArray(result.warning_details) ? result.warning_details : []),
      ...(Array.isArray(result.failures) ? result.failures : []),
      result.error || '',
    ]);
    const summary = `${result.message || '剧集图替换请求已完成'}；已有图删除 ${result.deleted_count || 0} 集，原本无图 ${result.missing_count || 0} 集。`;
    if (response.status === 207 || Number(result.warnings) > 0 || details.length) {
      mediaOperationFeedback.value = { type: 'warning', message: '剧集图处理有提醒', details: details.length ? details : [summary] };
      message.warning(summary);
    } else {
      message.success(summary);
      imageRefreshTokens.value.poster = Date.now();
    }
  } catch (error) {
    if (!isCurrentItem()) return;
    const result = error.response?.data || {};
    const summary = result.message || result.error || '替换剧集图失败，请检查日志。';
    mediaOperationFeedback.value = { type: 'error', message: summary, details: mediaOperationDetails([
      ...(Array.isArray(result.warning_details) ? result.warning_details : []),
      ...(Array.isArray(result.failures) ? result.failures : []), result.error || summary,
    ]) };
    message.error(summary);
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
const mediaInfoEditorBooted = ref(false);
const isSavingMediaInfo = ref(false);
const isFillingMediaInfoTitles = ref(false);
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
const mediaRecoveryState = ref({ pending: false, records: [] });
const mediaRecoveryRecordsFor = (mediaId) => {
  const id = String(mediaId || '').trim();
  return id ? mediaRecoveryState.value.records.filter(record => [record.target_ids, record.related_item_ids]
    .some(targets => Array.isArray(targets) && targets.some(target => String(target) === id))) : [];
};
const pageMediaRecoveryRecords = computed(() => mediaRecoveryRecordsFor(itemId.value));
const editorMediaRecoveryRecords = computed(() => mediaRecoveryRecordsFor(currentEditMediaId.value));
const isRecoveryCleanupOnly = records => records.length > 0 && records.every(record => record.kind === 'verified-cleanup');
const mediaRecoveryError = ref('');
const isCheckingMediaRecovery = ref(false);
const isRecoveringMediaInfo = ref(false);
const isConfirmingMediaRecovery = ref(false);
const mediaRecoveryBlocked = computed(() => mediaRecoveryState.value.pending || Boolean(mediaRecoveryError.value) || isCheckingMediaRecovery.value || isRecoveringMediaInfo.value);
const isExtractingCredits = ref(false);
const creditsExtractionFeedback = ref(null);
const creditsExtractionBlocked = computed(() => mediaRecoveryBlocked.value || isBackgroundTaskRunning.value
  || isSaving.value || isSavingMediaInfo.value || isRepairingTruehdCore.value || isDeletingMediaVersion.value
  || isReplacingEpisodeImages.value || isExtractingCredits.value);
let mediaRecoveryLoadSequence = 0;
let mediaRecoverySession = 0;
let mediaRecoveryDialog = null;
let mediaInfoLoadedSnapshot = '';
const currentEditMediaId = ref(null); // ★ 新增：记录当前正在编辑的具体媒体ID (电影ID或具体的集ID)
const mediaInfoVersions = ref([]);
let mediaInfoStreamSnapshotMap = new WeakMap();
let mediaInfoLoadSequence = 0;
const deletedMediaStreamSnapshots = ref([]);
const mediaInfoLanguageSelections = ref({});
const mediaInfoIsoLanguageOptions = ref([]);
const mediaInfoLanguageSelectRefs = new Map();
const mediaInfoLanguageUndoStack = [];
const closeMediaInfoEditor = () => {
  ++mediaInfoLoadSequence;
  showMediaInfoEditor.value = false;
  for (const instance of mediaInfoLanguageSelectRefs.values()) {
    instance.menu = false;
    instance.blur?.();
  }
};

const mergeCreditsChapter = (info, chapter) => {
  if (!chapter || chapter.MarkerType !== 'CreditsStart') return;
  for (const entry of Array.isArray(info) ? info : [info]) {
    if (!entry || typeof entry !== 'object') continue;
    entry.Chapters = [...(Array.isArray(entry.Chapters) ? entry.Chapters : [])
      .filter(item => item?.MarkerType !== 'CreditsStart'), { ...chapter }]
      .sort((left, right) => Number(left.StartPositionTicks || 0) - Number(right.StartPositionTicks || 0));
  }
};

const extractEpisodeCredits = async (id) => {
  const targetId = String(id || '').trim();
  if (!targetId || isExtractingCredits.value || creditsExtractionBlocked.value || !['Series', 'Season', 'Episode'].includes(itemDetails.value?.item_type)) return;
  const pageSequence = mediaDetailsLoadSequence;
  const editorSequence = mediaInfoLoadSequence;
  isExtractingCredits.value = true;
  creditsExtractionFeedback.value = null;
  const loadingMsg = message.loading('正在提取片尾，可在任务中心查看进度...', { duration: 0 });
  try {
    const { data: result, status } = await axios.post(`/api/media/${encodeURIComponent(targetId)}/extract_credits`, {});
    if (pageSequence !== mediaDetailsLoadSequence) return;
    const details = mediaOperationDetails([
      ...(Array.isArray(result.warning_details) ? result.warning_details : []),
      ...(Array.isArray(result.failures) ? result.failures : []),
      ...(Array.isArray(result.items) ? result.items.filter(item => item.status === 'skipped' && !item.chapter && item.error) : []),
    ]);
    const hasWarnings = status === 207 || Number(result.failed) > 0 || Number(result.warnings) > 0 || details.length > 0;
    const summary = result.message || (hasWarnings ? '片尾提取有提醒' : '片尾提取已完成');
    creditsExtractionFeedback.value = { itemId: targetId, type: hasWarnings ? 'warning' : 'success', message: summary, details };
    if (editorSequence === mediaInfoLoadSequence && targetId === String(currentEditMediaId.value || '')) {
      const chapter = result.chapter || result.items?.find(item => String(item.item_id) === targetId && item.chapter?.MarkerType === 'CreditsStart')?.chapter;
      mergeCreditsChapter(mediaInfoData.value, chapter);
    }
    if (hasWarnings) message.warning(summary);
    else message.success(summary);
  } catch (error) {
    if (pageSequence !== mediaDetailsLoadSequence) return;
    const result = error.response?.data || {};
    const summary = result.error || result.message || '片尾提取失败，请检查日志。';
    creditsExtractionFeedback.value = { itemId: targetId, type: 'error', message: summary, details: mediaOperationDetails([
      ...(Array.isArray(result.warning_details) ? result.warning_details : []),
      ...(Array.isArray(result.failures) ? result.failures : []),
    ]) };
    message.error(summary);
  } finally {
    loadingMsg.destroy();
    isExtractingCredits.value = false;
  }
};

const loadMediaRecovery = async () => {
  const sequence = ++mediaRecoveryLoadSequence;
  const session = mediaRecoverySession;
  isCheckingMediaRecovery.value = true;
  try {
    const { data } = await axios.get('/api/media_info/recovery');
    if (session !== mediaRecoverySession || sequence !== mediaRecoveryLoadSequence) return false;
    if (typeof data?.pending !== 'boolean' || !Array.isArray(data.records)) throw new Error('恢复状态响应无效，请重新检查');
    mediaRecoveryState.value = data;
    mediaRecoveryError.value = '';
    return true;
  } catch (error) {
    if (session === mediaRecoverySession && sequence === mediaRecoveryLoadSequence) {
      mediaRecoveryError.value = error.response?.data?.error || error.message || '无法读取恢复状态，请重新检查';
    }
    return false;
  } finally {
    if (session === mediaRecoverySession && sequence === mediaRecoveryLoadSequence) isCheckingMediaRecovery.value = false;
  }
};

const recoveryConflictValue = (value) => {
  const text = value === undefined ? '（无该字段）' : typeof value === 'string' ? value : JSON.stringify(value);
  return text.length > 300 ? `${text.slice(0, 300)}…（${text.length} 字符）` : text;
};

const recoveryJsonConflictHashes = (record) => {
  const hashes = {};
  for (const conflict of Array.isArray(record.json_conflicts) ? record.json_conflicts : []) {
    if (!conflict.item_id || !/^[a-f0-9]{64}$/i.test(conflict.current_sha256 || '')) return null;
    const hash = conflict.current_sha256.toLowerCase();
    if (hashes[conflict.item_id] && hashes[conflict.item_id] !== hash) return null;
    hashes[conflict.item_id] = hash;
  }
  return Object.fromEntries(Object.entries(hashes).sort());
};

const recoverMediaInfo = async (record, currentJsonHashes = null) => {
  const currentRecord = mediaRecoveryState.value.records.find(item => item.id === record.id && item.can_recover);
  if (isRecoveringMediaInfo.value || isSavingMediaInfo.value || !currentRecord) return;
  const actualHashes = recoveryJsonConflictHashes(currentRecord);
  if ((currentRecord.json_conflicts?.length && !currentJsonHashes) || (currentJsonHashes && JSON.stringify(currentJsonHashes) !== JSON.stringify(actualHashes))) {
    mediaRecoveryError.value = '当前 JSON 已变化或缺少有效哈希，请重新检查差异并确认备份后恢复';
    message.warning(mediaRecoveryError.value);
    return;
  }
  const session = mediaRecoverySession;
  const sequence = mediaInfoLoadSequence;
  const targetId = String(currentEditMediaId.value || '');
  const draftSnapshot = JSON.stringify(mediaInfoData.value);
  isRecoveringMediaInfo.value = true;
  mediaRecoveryError.value = '';
  try {
    const payload = { recovery_id: record.id };
    if (currentJsonHashes) payload.current_json_hashes = { ...currentJsonHashes };
    const { data, status } = await axios.post('/api/media_info/recovery', payload);
    if (session !== mediaRecoverySession) return;
    if (data?.recovered_id !== record.id || typeof data.pending !== 'boolean' || typeof data.backup_deleted !== 'boolean') throw new Error(data?.error || data?.message || '恢复或备份清理尚未通过验收');
    if (!await loadMediaRecovery() || session !== mediaRecoverySession) return;
    const warnings = mediaOperationDetails(Array.isArray(data.warning_details) ? data.warning_details : []);
    const preservedDirectories = [...new Set(Array.isArray(data.conflict_backup_directories) ? data.conflict_backup_directories : [])].filter(directory => typeof directory === 'string' && directory);
    const preservedDetails = preservedDirectories.map(directory => `当前 JSON 已另行备份并保留：${directory}`);
    if (preservedDetails.length) mediaOperationFeedback.value = { type: 'info', message: '当前 JSON 备份已保留', details: preservedDetails };
    else if (mediaOperationFeedback.value?.message === '当前 JSON 备份已保留') mediaOperationFeedback.value = null;
    if (status === 207 || !data.backup_deleted) {
      if (!warnings.length) warnings.push(data.message || `恢复备份尚未删除：${record.directory}`);
      mediaRecoveryError.value = warnings.join('；');
      mediaOperationFeedback.value = { type: 'warning', message: '恢复备份尚未删除，请重试清理', details: [...warnings, ...preservedDetails] };
      if (sequence === mediaInfoLoadSequence && targetId === String(currentEditMediaId.value || '')) mediaInfoSaveFeedback.value = mediaOperationFeedback.value;
      message.warning(mediaOperationFeedback.value.message);
      return;
    }
    if (Number(data.warnings) > 0 && !warnings.length) warnings.push(data.message || `恢复已验收，备份收尾尚未完成：${record.directory}`);
    if (warnings.length) mediaOperationFeedback.value = { type: 'warning', message: '恢复已通过验收，仍有收尾提醒', details: warnings };
    if (mediaRecoveryRecordsFor(itemId.value).length || mediaRecoveryRecordsFor(currentEditMediaId.value).length) {
      message.warning('本条记录已恢复，仍有其他待恢复记录，请继续处理');
      if (warnings.length && sequence === mediaInfoLoadSequence) mediaInfoSaveFeedback.value = mediaOperationFeedback.value;
      return;
    }
    if (mediaOperationFeedback.value?.message === '恢复备份尚未删除，请重试清理') mediaOperationFeedback.value = null;
    if (mediaInfoSaveFeedback.value?.message === '恢复备份尚未删除，请重试清理') mediaInfoSaveFeedback.value = null;
    if (!preservedDetails.length && mediaInfoSaveFeedback.value?.message === '当前 JSON 备份已保留') mediaInfoSaveFeedback.value = null;
    if (warnings.length) message.warning(mediaOperationFeedback.value.message);
    else message.success(data.message || (record.kind === 'verified-cleanup' ? '恢复备份已清理' : '媒体信息与原神医配置已验收，恢复备份已删除'));
    if (record.kind === 'verified-cleanup') {
      if (preservedDetails.length && showMediaInfoEditor.value && sequence === mediaInfoLoadSequence) mediaInfoSaveFeedback.value = { type: 'info', message: '当前 JSON 备份已保留', details: preservedDetails };
      return;
    }
    if (showMediaInfoEditor.value && sequence === mediaInfoLoadSequence && targetId === String(currentEditMediaId.value || '')) {
      const preserveDraft = draftSnapshot !== mediaInfoLoadedSnapshot || draftSnapshot !== JSON.stringify(mediaInfoData.value);
      if (!preserveDraft) {
        await openMediaInfoEditor(targetId);
        if (session !== mediaRecoverySession || mediaInfoLoadSequence !== sequence + 1 || targetId !== String(currentEditMediaId.value || '') || !showMediaInfoEditor.value) return;
      }
      if (preserveDraft || warnings.length || preservedDetails.length) mediaInfoSaveFeedback.value = {
        type: warnings.length ? 'warning' : 'info',
        message: warnings.length ? '恢复已通过验收，仍有收尾提醒' : preserveDraft ? '恢复已完成，当前编辑草稿已保留' : '当前 JSON 备份已保留',
        details: [...warnings, ...preservedDetails, ...(preserveDraft ? ['当前编辑草稿已保留；可重新打开编辑器读取恢复后的媒体信息。'] : [])],
      };
    }
  } catch (error) {
    if (session !== mediaRecoverySession) return;
    const reason = error.response?.data?.error || error.message || '恢复失败，请检查恢复目录';
    const checked = error.response ? await loadMediaRecovery() : true;
    if (session !== mediaRecoverySession) return;
    mediaRecoveryError.value = [reason, ...(!checked && mediaRecoveryError.value ? [mediaRecoveryError.value] : [])].join('；');
    message.error(mediaRecoveryError.value);
  } finally {
    isRecoveringMediaInfo.value = false;
  }
};

const confirmMediaRecovery = (record) => {
  if (isConfirmingMediaRecovery.value || isRecoveringMediaInfo.value || !record.can_recover) return;
  const session = mediaRecoverySession;
  const conflict = Boolean(record.json_conflicts?.length);
  const hashes = conflict ? recoveryJsonConflictHashes(record) : null;
  if (conflict && !hashes) {
    mediaRecoveryError.value = '当前 JSON 缺少有效哈希，请重新检查后恢复';
    return;
  }
  isConfirmingMediaRecovery.value = true;
  const cleanup = record.kind === 'verified-cleanup';
  mediaRecoveryDialog = dialog.warning({
    title: cleanup ? '清理恢复备份' : conflict ? '备份当前 JSON 并恢复' : '恢复媒体信息',
    content: cleanup ? `媒体信息与原神医配置已通过验收。本次将删除已验收的恢复备份及关联的冲突 JSON 备份。当前编辑草稿会保留。\n备份目录：${record.directory}` : conflict ? `当前 JSON 已发生变化。将先把以下当前文件备份到独立留存目录，再恢复原请求 JSON；完整验收后删除原恢复备份及关联的冲突 JSON 备份。验收失败时保留全部备份。当前编辑草稿会保留。\n${record.json_conflicts.map(item => `${item.item_id}：${item.path}\nSHA256：${hashes[item.item_id]}`).join('\n')}` : `将核对备份与 Emby 数据并恢复原神医配置，验收成功后删除原恢复备份及关联的冲突 JSON 备份。当前未保存的草稿会保留。\n恢复目录：${record.directory}`,
    positiveText: cleanup ? '开始清理' : conflict ? '备份并恢复' : '开始恢复', negativeText: '取消',
    onPositiveClick: () => session === mediaRecoverySession ? recoverMediaInfo(record, hashes) : undefined,
    onClose: () => { isConfirmingMediaRecovery.value = false; mediaRecoveryDialog = null; },
  });
};

// ★ 新增：剧集分集选择相关状态
const showEpisodeSelector = ref(false);
let episodeSelectorLoadSequence = 0;
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
    if (Number(res.data.warnings) > 0) message.warning(res.data.message || '重命名完成，仍有处理提醒');
    else message.success(res.data.message || '媒体已重命名');
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
        is_forced: stream.Type === 'Subtitle' && typeof stream.IsForced === 'boolean' ? stream.IsForced : null,
        is_hearing_impaired: stream.Type === 'Subtitle' && typeof stream.IsHearingImpaired === 'boolean' ? stream.IsHearingImpaired : null,
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
  return Number(ep?.version_count || 0) > 1
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

const selectEpisodeBatch = (episodes) => {
  if (loadingConfig.value) return;
  const skippedIds = new Set();
  selectedEpisodeIds.value = episodes
    .filter(ep => {
      const skip = configModel.value?.media_edit_skip_track_differences_on_select_all === true
        && ep.track_status === 'inconsistent';
      const id = String(ep.emby_id || '').trim();
      if (skip && id) skippedIds.add(id);
      return !skip;
    })
    .map(ep => String(ep.emby_id || '').trim())
    .filter(Boolean);
  applyToSelectedEpisodes.value = selectedEpisodeIds.value.length > 0;
  if (skippedIds.size) message.info(`已跳过 ${skippedIds.size} 个缺轨或额外轨媒体，可单独勾选。`);
};

const selectAllEpisodes = () => selectEpisodeBatch(visibleSeasonEpisodes.value);

const selectEpisodesByVersion = () => {
  if (selectedBatchVersionIndex.value === null) return;
  selectEpisodeBatch(visibleSeasonEpisodes.value
    .filter(ep => Number(ep.version_index || 1) === Number(selectedBatchVersionIndex.value))
  );
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
    const sequence = ++episodeSelectorLoadSequence;
    const pageSequence = mediaDetailsLoadSequence;
    const targetId = String(itemId.value || '');
    const isCurrentRequest = () => sequence === episodeSelectorLoadSequence
      && pageSequence === mediaDetailsLoadSequence && targetId === String(itemId.value || '');
    // 如果是剧集或季，先拉取分集列表并打开选择框
    showEpisodeSelector.value = true;
    isFetchingEpisodes.value = true;
    clearSelectedEpisodes();
    try {
      const res = await axios.get(`/api/media_info/series/${targetId}/episodes`);
      if (!isCurrentRequest() || !showEpisodeSelector.value) return;
      episodesList.value = res.data;
      selectedSeasonNumber.value = episodeSeasonGroups.value[0]?.season_number ?? null;
      selectedBatchVersionIndex.value = null;
    } catch (e) {
      if (!isCurrentRequest() || !showEpisodeSelector.value) return;
      message.error(e.response?.data?.error || "获取分集列表失败");
      showEpisodeSelector.value = false;
    } finally {
      if (isCurrentRequest()) isFetchingEpisodes.value = false;
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
  const options = stream.Type === 'Audio' ? audioLanguageOptions.value : subtitleLanguageOptions.value;
  if (findMediaInfoLanguageOption(options, language)) return;
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

let mediaInfoLanguageMappingPromise = null;
const fetchLanguageMapping = () => {
  if (mediaInfoLanguageMappingPromise) return mediaInfoLanguageMappingPromise;
  mediaInfoLanguageMappingPromise = axios.get('/api/mappings/config/language_mapping').then(res => {
    const rawData = Array.isArray(res.data) ? res.data : [];
    audioLanguageOptions.value = buildMediaInfoLanguageOptions(rawData);
    subtitleLanguageOptions.value = buildMediaInfoLanguageOptions(rawData, true);
  }).catch(e => {
    console.error("获取语言映射失败", e);
  }).finally(() => { mediaInfoLanguageMappingPromise = null; });
  return mediaInfoLanguageMappingPromise;
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
  return resolveMediaInfoLanguageSelection(options, stream);
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

const fillLanguagesToTitles = async () => {
  if (isFillingMediaInfoTitles.value || isSavingMediaInfo.value || isRepairingTruehdCore.value || isDeletingMediaVersion.value) return;
  const targetId = String(currentEditMediaId.value || '');
  const sequence = mediaInfoLoadSequence;
  const streams = [...displayStreams.value];
  const requestStreams = JSON.parse(JSON.stringify(streams));
  const snapshot = JSON.stringify({ streams: mediaStreams.value, selections: mediaInfoLanguageSelections.value });
  isFillingMediaInfoTitles.value = true;
  try {
    const response = await axios.post('/api/media_info/fill_titles', { streams: requestStreams });
    if (!showMediaInfoEditor.value || targetId !== String(currentEditMediaId.value || '') || sequence !== mediaInfoLoadSequence) return;
    if (snapshot !== JSON.stringify({ streams: mediaStreams.value, selections: mediaInfoLanguageSelections.value })) {
      message.info('编辑草稿已变化，请重新填充标题');
      return;
    }
    const updated = response.data?.streams;
    const report = response.data?.report;
    if (!Array.isArray(updated) || updated.length !== streams.length || !report
      || updated.some((stream, index) => stream?.Type !== streams[index].Type || stream?.Index !== streams[index].Index)) {
      throw new Error('标题填充结果与当前轨道不一致，请重试');
    }
    const selections = { ...mediaInfoLanguageSelections.value };
    streams.forEach((stream, index) => {
      const languageChanged = stream.Language !== updated[index].Language;
      for (const field of ['Title', 'Language']) {
        if (Object.hasOwn(updated[index], field)) stream[field] = updated[index][field];
        else delete stream[field];
      }
      if (languageChanged) {
        ensureMediaInfoLanguageOption(stream);
        selections[getMediaInfoStreamSelectionKey(stream)] = resolveInitialMediaInfoLanguageSelection(stream);
      }
    });
    mediaInfoLanguageSelections.value = selections;
    const changed = Number(report.changed || 0);
    const skipped = Array.isArray(report.skipped_tracks) ? report.skipped_tracks.length : 0;
    const pending = Array.isArray(report.pending) ? report.pending.length : 0;
    const text = `${changed ? `已填充 ${changed} 条轨道的语言和标题` : '轨道标题无需修改'}${skipped ? `，已跳过 ${skipped} 条中文字幕` : ''}${pending ? `，${pending} 条轨道语言未知或存在歧义，需手动确认` : ''}`;
    if (pending) message.warning(text);
    else if (changed) message.success(text);
    else message.info(text);
  } catch (error) {
    message.error(error.response?.data?.error || error.response?.data?.message || error.message || '填充标题失败');
  } finally {
    isFillingMediaInfoTitles.value = false;
  }
};

// 2. 打开编辑器并获取数据 (修改为接收 targetId 参数)
const openMediaInfoEditor = async (targetId) => {
  const sequence = ++mediaInfoLoadSequence;
  mediaInfoSaveFeedback.value = null;
  loadMediaRecovery();

  const loadingMsg = message.loading("正在读取底层媒体指纹...", { duration: 0 });
  try {
    const [res] = await Promise.all([
      axios.get(`/api/media_info/edit/${targetId}`),
      audioLanguageOptions.value.length === 0 ? fetchLanguageMapping() : Promise.resolve(),
    ]);
    if (sequence !== mediaInfoLoadSequence) return;
    mediaInfoLanguageSelectRefs.clear();
    currentEditMediaId.value = String(targetId);
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

    mediaInfoLoadedSnapshot = JSON.stringify(mediaInfoData.value);
    mediaInfoEditorBooted.value = true;
    showMediaInfoEditor.value = true;
  } catch (e) {
    if (sequence === mediaInfoLoadSequence) {
      message.error(e.response?.data?.error || "获取媒体信息失败");
    }
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
  if (!targetId || mediaInfoVersionOptions.value.length <= 1 || isDeletingMediaVersion.value) return;
  const sequence = mediaDetailsLoadSequence;
  const session = mediaRecoverySession;
  const editorSequence = mediaInfoLoadSequence;
  const isCurrentEditor = () => sequence === mediaDetailsLoadSequence && session === mediaRecoverySession
    && editorSequence === mediaInfoLoadSequence
    && targetId === String(currentMediaInfoVersion.value?.emby_id || '');

  isDeletingMediaVersion.value = true;
  mediaOperationFeedback.value = null;
  const loadingMsg = message.loading('正在删除 115、STRM 和 Emby 当前版本...', { duration: 0 });
  try {
    const response = await axios.post(`/api/media_info/delete_version/${targetId}`);
    if (!isCurrentEditor()) return;
    const result = response.data || {};
    const remainingItemId = String(result.remaining_item_id || '').trim();
    const deletedItemId = String(result.deleted_item_id || '').trim();
    const details = mediaOperationDetails([
      ...(Array.isArray(result.warning_details) ? result.warning_details : []),
      ...(Array.isArray(result.failures) ? result.failures : []), result.error || '',
    ]);
    if (remainingItemId === targetId || deletedItemId && deletedItemId !== targetId) details.push('删除结果中的媒体身份不一致，未切换到返回的版本，请检查任务结果');
    if (deletedItemId === targetId && !remainingItemId) details.push('当前版本已删除，但未返回可验证的剩余版本，请检查任务结果');
    if (!deletedItemId && !remainingItemId && !details.length && !result.warnings && response.status !== 207) details.push('未确认删除后的剩余版本，请检查任务结果');
    const hasWarnings = response.status === 207 || Number(result.warnings) > 0 || details.length > 0;
    const summary = result.message || (hasWarnings ? '删除版本有提醒，请检查处理结果' : '当前版本已删除并刷新');
    if (hasWarnings) {
      mediaOperationFeedback.value = { type: 'warning', message: summary, details: details.length ? details : [summary] };
      message.warning(summary);
    } else {
      message.success(summary);
    }
    const deleted = deletedItemId === targetId || !deletedItemId && !hasWarnings;
    if (!deleted) return;
    showMediaInfoEditor.value = false;

    if (remainingItemId && remainingItemId !== targetId) {
      itemId.value = remainingItemId;
      currentEditMediaId.value = remainingItemId;
      renameTargetId.value = remainingItemId;
      await router.replace({ name: 'MediaEditPage', params: { itemId: remainingItemId } });
      await fetchMediaDetails();
    }
  } catch (error) {
    if (!isCurrentEditor()) return;
    const result = error.response?.data || {};
    const summary = result.message || result.error || '删除当前版本失败，请检查日志';
    mediaOperationFeedback.value = { type: 'error', message: summary, details: mediaOperationDetails([
      ...(Array.isArray(result.warning_details) ? result.warning_details : []),
      ...(Array.isArray(result.failures) ? result.failures : []), result.error || summary,
    ]) };
    message.error(summary);
  } finally {
    loadingMsg.destroy();
    isDeletingMediaVersion.value = false;
  }
};

// 3. 保存修改 (修改为使用 currentEditMediaId)
const saveMediaInfo = async () => {
  const targetId = String(currentEditMediaId.value || '').trim();
  if (!targetId || isFillingMediaInfoTitles.value || isSavingMediaInfo.value || isRepairingTruehdCore.value || isDeletingMediaVersion.value || mediaRecoveryBlocked.value) return;
  const sequence = mediaInfoLoadSequence;
  const draftSnapshot = JSON.stringify(mediaInfoData.value);
  const isCurrentEditor = () => showMediaInfoEditor.value
    && sequence === mediaInfoLoadSequence
    && targetId === String(currentEditMediaId.value || '');
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
    const res = await axios.post(`/api/media_info/edit/${targetId}`, payload);
    if (!isCurrentEditor()) return;
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
          track_difference: `本次批量保存跳过 ${reasons.join('；')}`
        };
      });
    }
    const metadata = res.data.metadata_refresh || {};
    const thumbnails = res.data.video_thumbnails || {};
    const details = [
      ...(Array.isArray(res.data.warning_details) ? res.data.warning_details : []),
      ...(Array.isArray(res.data.failures) ? res.data.failures : []),
      ...skippedReports.flatMap(report => report.skipped.map(item => ({
        item_id: [report.item_id, report.path].filter(Boolean).join(' · '), reason: item.reason,
      }))),
      ...(Array.isArray(metadata.failures) ? metadata.failures : []),
    ];
    if (metadata.state && !['verified', 'completed'].includes(metadata.state)) {
      details.push(`Emby 刷新尚未通过验收${metadata.failed_ids?.length ? `（${metadata.failed_ids.join('、')}）` : ''}`);
    }
    if (Number(thumbnails.failed) > 0) details.push(thumbnails.reason || thumbnails.message || '视频缩略图处理未完成');
    const draftUnchanged = draftSnapshot === JSON.stringify(mediaInfoData.value);
    if (!draftUnchanged) details.push('保存期间草稿已修改，已保留新草稿，请确认后重新保存');
    if (Number(res.data.warnings) > 0 && !details.length) details.push(res.data.message || '部分处理未完成，请检查任务结果');
    const sourceVerified = res.data.media_info_saved === true
      && Array.isArray(res.data.saved_item_ids) && res.data.saved_item_ids.map(String).includes(targetId)
      && ['verified', 'completed'].includes(metadata.state)
      && Array.isArray(metadata.verified_ids) && metadata.verified_ids.map(String).includes(targetId)
      && !(metadata.failed_ids || []).map(String).includes(targetId)
      && ![...(res.data.failures || []), ...(metadata.failures || [])].some(failure => !failure?.item_id || String(failure.item_id) === targetId)
      && !skippedReports.some(report => String(report.item_id) === targetId);
    const savedInfo = res.data.saved_mediainfo;
    const currentInfo = Array.isArray(mediaInfoData.value) ? mediaInfoData.value : [mediaInfoData.value];
    const streamsFor = entry => (entry?.MediaSourceInfo || entry)?.MediaStreams;
    const canonicalMatches = Array.isArray(savedInfo) && savedInfo.length === currentInfo.length
      && savedInfo.every((entry, index) => {
        const updated = streamsFor(entry), current = streamsFor(currentInfo[index]);
        return Array.isArray(updated) && Array.isArray(current) && updated.length === current.length
          && updated.every((stream, position) => stream && typeof stream === 'object' && !Array.isArray(stream)
            && current[position] && stream.Type === current[position].Type && stream.Index === current[position].Index);
      });
    if (sourceVerified && savedInfo != null && !canonicalMatches) details.push('保存结果与当前轨道不一致，已保留当前草稿，请重新读取媒体信息');
    if (sourceVerified && draftUnchanged && canonicalMatches) {
      const selections = { ...mediaInfoLanguageSelections.value };
      savedInfo.forEach((entry, index) => {
        const updated = streamsFor(entry), current = streamsFor(currentInfo[index]);
        updated.forEach((savedStream, position) => {
          const stream = current[position], languageChanged = stream.Language !== savedStream.Language;
          for (const field of Object.keys(stream)) if (!Object.hasOwn(savedStream, field)) delete stream[field];
          Object.assign(stream, savedStream);
          updated[position] = stream;
          if (languageChanged && ['Audio', 'Subtitle'].includes(stream.Type)) {
            ensureMediaInfoLanguageOption(stream);
            selections[getMediaInfoStreamSelectionKey(stream)] = resolveInitialMediaInfoLanguageSelection(stream);
          }
        });
      });
      mediaInfoData.value = savedInfo;
      mediaInfoLanguageSelections.value = selections;
    }
    if (details.length) {
      mediaInfoSaveFeedback.value = {
        type: 'warning', message: '保存有提醒，请检查处理结果', details: mediaOperationDetails(details),
      };
      message.warning(mediaInfoSaveFeedback.value.message);
      loadMediaRecovery();
    } else {
      message.success(res.data.message || "媒体信息已更新！");
    }
    if (draftUnchanged && (!details.length || sourceVerified) && (savedInfo == null || !sourceVerified || canonicalMatches)) {
      mediaInfoLoadedSnapshot = JSON.stringify(mediaInfoData.value);
      const snapshots = createMediaStreamSnapshots(mediaStreams.value);
      mediaInfoStreamSnapshotMap = new WeakMap(mediaStreams.value.map((stream, index) => [stream, snapshots[index]]));
      deletedMediaStreamSnapshots.value = [];
      mediaInfoLanguageUndoStack.length = 0;
    }
  } catch (e) {
    if (!isCurrentEditor()) return;
    const result = e.response?.data || {};
    mediaInfoSaveFeedback.value = {
      type: 'error', message: result.message || result.error || '保存失败',
      details: mediaOperationDetails(Array.isArray(result.failures) && result.failures.length ? result.failures : [result.error || '请检查后端日志']),
    };
    message.error(mediaInfoSaveFeedback.value.message);
    loadMediaRecovery();
  } finally {
    loadingMsg.destroy();
    isSavingMediaInfo.value = false;
  }
};

const repairCurrentAudioTracks = async () => {
  const targetId = String(currentEditMediaId.value || '').trim();
  if (!targetId || isRepairingTruehdCore.value) return;
  const sequence = mediaInfoLoadSequence;
  const isCurrentEditor = () => showMediaInfoEditor.value
    && sequence === mediaInfoLoadSequence
    && targetId === String(currentEditMediaId.value || '');

  isRepairingTruehdCore.value = true;
  const loadingMsg = message.loading('正在检查当前媒体音轨，歧义时将只读探测 CD2...', { duration: 0 });
  let repaired = false;
  try {
    const res = await axios.post(`/api/media_info/repair_truehd_core/${targetId}`);
    if (!isCurrentEditor()) return;
    const report = res.data?.result || res.data;
    repaired = Number(report?.repaired || 0) > 0;
    const hasWarnings = Number(report?.warnings || res.data?.warnings || 0) > 0 || Number(report?.ambiguous || 0) > 0;
    const summary = res.data?.message || report?.message || '';
    const reason = report?.ambiguous_items?.[0]?.reason;
    const feedback = [summary, reason && !summary.includes(reason) ? reason : ''].filter(Boolean).join('；');
    if (repaired) {
      if (hasWarnings) message.warning(feedback || '音轨已纠正，仍有处理提醒');
      else message.success(feedback || '当前媒体音轨已纠正');
    } else if (hasWarnings) {
      message.warning(feedback || '音轨结构尚未能唯一确认，已保留原数据');
    } else {
      message.info(feedback || '当前媒体无需纠正');
    }
  } catch (error) {
    if (!isCurrentEditor()) return;
    const report = error.response?.data || {};
    const summary = report.message || report.error || '纠正当前媒体音轨失败';
    const reason = (report.result || report).failed_items?.[0]?.reason;
    const errorMessage = [summary, reason && !summary.includes(reason) ? reason : ''].filter(Boolean).join('；');
    if (error.response?.status === 409) message.warning(errorMessage);
    else message.error(errorMessage);
  } finally {
    loadingMsg.destroy();
    isRepairingTruehdCore.value = false;
  }

  if (repaired && isCurrentEditor()) {
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
let tmdbImagesLoadSequence = 0;
const currentTmdbImages = ref([]); // 当前展示的图片列表
const currentTmdbImageLabel = ref('');
const currentTmdbImageAspect = ref('2/3');

// 打开 TMDb 选图框
const openTmdbSelector = async (type, tmdbKey) => {
  const sequence = ++tmdbImagesLoadSequence;
  const pageSequence = mediaDetailsLoadSequence;
  const targetId = String(itemId.value || '');
  const ownsRequest = () => sequence === tmdbImagesLoadSequence
    && pageSequence === mediaDetailsLoadSequence && targetId === String(itemId.value || '')
    && type === currentEditImageType.value;
  const isCurrentSelector = () => ownsRequest() && showTmdbSelector.value;
  currentEditImageType.value = type;

  // 设置 UI 标题和比例
  const imgConfig = imageTypes.find(i => i.type === type);
  currentTmdbImageLabel.value = imgConfig.label;
  currentTmdbImageAspect.value = imgConfig.aspect;

  showTmdbSelector.value = true;
  isFetchingTmdbImages.value = false;

  // 如果还没拉取过，去后端拉取
  if (!tmdbImagesCache.value) {
    isFetchingTmdbImages.value = true;
    try {
      const res = await axios.get(`/api/tmdb_images/${targetId}`);
      if (!isCurrentSelector()) return;
      tmdbImagesCache.value = res.data;
    } catch (error) {
      if (!isCurrentSelector()) return;
      message.error("获取 TMDb 图片失败");
      showTmdbSelector.value = false;
      return;
    } finally {
      if (ownsRequest()) isFetchingTmdbImages.value = false;
    }
  }

  // 根据类型展示对应的图片列表
  if (isCurrentSelector()) currentTmdbImages.value = tmdbImagesCache.value[tmdbKey] || [];
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
  const colors = ['primary', 'secondary', 'success', 'info', 'warning', 'error']
    .map(color => theme.current.value.colors[color]);
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
  if (isTranslating.value) return;
  const targetId = String(itemId.value || '');
  const requestedItem = itemDetails.value;
  const castSnapshot = JSON.stringify(editableCast.value);
  isTranslating.value = true;
  try {
    const payload = {
      cast: editableCast.value,
      title: itemDetails.value.item_name,
      year: itemDetails.value.production_year,
      media_type: itemDetails.value.item_type,
    };

    const response = await axios.post('/api/actions/translate_cast_sa', payload);
    if (targetId !== String(itemId.value || '') || requestedItem !== itemDetails.value) return;
    if (castSnapshot !== JSON.stringify(editableCast.value)) {
      message.warning('翻译期间演员列表已修改，已保留当前草稿，请确认后重新翻译。');
      return;
    }
    const translatedList = response.data;

    editableCast.value = translatedList.map((actor, index) => ({
      ...actor,
      _temp_id: `translated-actor-${Date.now()}-${index}`
    }));

    const incomplete = editableCast.value.filter(actor => actor.matchStatus === '翻译未完成' || actor.translationWarning);
    if (editableCast.value.some(actor => actor.matchStatus === 'AI未启用')) {
      message.warning('AI 翻译未启用，已保留原演员数据，请先开启 AI 翻译。');
    } else if (incomplete.length) {
      const reasons = [...new Set(incomplete.map(actor => actor.translationWarning).filter(Boolean))];
      message.warning([`${incomplete.length} 位演员的姓名或角色翻译未完成，已保留取得的译文`, ...reasons].join('；'), { duration: 8000 });
    } else {
      message.success("智能翻译完成！");
    }

  } catch (error) {
    console.error("一键翻译失败:", error);
    message.error(error.response?.data?.error || "翻译失败，请检查后端日志。");
  } finally {
    isTranslating.value = false;
  }
};

const fetchMediaDetails = async () => {
  const sequence = ++mediaDetailsLoadSequence;
  const targetId = String(itemId.value || '');
  isLoading.value = true;
  try {
    const response = await axios.get(`/api/media_for_editing/${targetId}`);
    if (sequence !== mediaDetailsLoadSequence) return;
    const redirectItemId = String(response.data?.redirect_item_id || '').trim();
    if (redirectItemId && redirectItemId !== String(itemId.value || '')) {
      itemId.value = redirectItemId;
      currentEditMediaId.value = redirectItemId;
      renameTargetId.value = redirectItemId;
      await router.replace({ name: 'MediaEditPage', params: { itemId: redirectItemId } });
    }
    if (sequence !== mediaDetailsLoadSequence) return;
    itemDetails.value = response.data;

    if (response.data && response.data.search_links) {
      searchLinks.value = response.data.search_links;
    }

  } catch (error) {
    if (sequence !== mediaDetailsLoadSequence) return;
    message.error(error.response?.data?.error || "获取媒体详情失败。");
    itemDetails.value = null;
  } finally {
    if (sequence === mediaDetailsLoadSequence) isLoading.value = false;
  }
};

watch(() => route.fullPath, () => {
  invalidateActorSaveSession();
  ++mediaRecoverySession;
  mediaRecoveryDialog?.destroy();
});

watch(() => route.params.itemId, (value) => {
  const targetId = String(value || '');
  if (!targetId || targetId === String(itemId.value || '')) return;
  closeMediaInfoEditor();
  showEpisodeSelector.value = false;
  showRenameEditor.value = false;
  showImageEditor.value = false;
  showUrlPrompt.value = false;
  showAddActorModal.value = false;
  showTmdbSelector.value = false;
  imagePreview.value = null;
  tmdbImagesCache.value = null;
  mediaOperationFeedback.value = null;
  mediaRecoveryError.value = '';
  creditsExtractionFeedback.value = null;
  debouncedSearchActors.cancel();
  itemId.value = targetId;
  fetchMediaDetails();
  loadMediaRecovery();
});

onMounted(() => {
  window.addEventListener('keydown', handleMediaInfoUndoShortcut);
  itemId.value = route.params.itemId;

  if (itemId.value) {
    fetchMediaDetails();
    loadMediaRecovery();
  } else {
    message.error("未提供媒体项ID！");
    isLoading.value = false;
  }
});

onBeforeUnmount(() => {
  invalidateActorSaveSession();
  ++mediaRecoverySession;
  mediaRecoveryDialog?.destroy();
  ++mediaDetailsLoadSequence;
  window.removeEventListener('keydown', handleMediaInfoUndoShortcut);
  closeMediaInfoEditor();
  mediaInfoLanguageSelectRefs.clear();
  debouncedSearchActors.cancel();
});

const goBack = () => {
  if (window.history.state?.back) {
    router.back();
    return;
  }
  router.push({ name: 'ReviewList' });
};

const handleSaveChanges = async () => {
  if (!itemDetails.value?.item_id || isSaving.value) return;
  invalidateActorSaveSession();
  const session = actorSaveSession;
  const targetId = String(itemDetails.value.item_id);
  const requestedItem = itemDetails.value;
  const draftSnapshot = JSON.stringify(editableCast.value);
  const isCurrentItem = () => session === actorSaveSession
    && route.name === 'MediaEditPage'
    && targetId === String(route.params.itemId || '')
    && requestedItem === itemDetails.value
    && targetId === String(itemDetails.value?.item_id || '');
  isSaving.value = true;
  try {
    await nextTick();
    if (!isCurrentItem()) return;

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

    await axios.post(`/api/update_media_cast_sa/${targetId}`, payload);
    if (!isCurrentItem()) return;
    if (draftSnapshot !== JSON.stringify(editableCast.value)) {
      message.warning('保存期间演员列表已修改，已保留当前草稿，请确认后重新保存。');
      return;
    }

    message.success("修改已保存，Emby将自动刷新。");
    const navigationTimer = setTimeout(() => {
      if (actorSaveNavigationTimer === navigationTimer) actorSaveNavigationTimer = null;
      if (isCurrentItem() && draftSnapshot === JSON.stringify(editableCast.value)) goBack();
    }, 1500);
    actorSaveNavigationTimer = navigationTimer;

  } catch (error) {
    if (!isCurrentItem()) return;
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
  border: 1px solid rgba(var(--v-border-color), var(--v-border-opacity));
  border-radius: 6px;
  overflow: hidden;
  background: rgb(var(--v-theme-surface));
}
.rename-result-title {
  padding: 10px 14px;
  border-bottom: 1px solid rgba(var(--v-border-color), var(--v-border-opacity));
  color: rgb(var(--v-theme-on-surface));
  font-size: 14px;
  font-weight: 600;
}
.rename-result-row {
  display: grid;
  grid-template-columns: 112px minmax(0, 1fr);
  align-items: stretch;
  border-bottom: 1px solid rgba(var(--v-border-color), var(--v-border-opacity));
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
  color: rgba(var(--v-theme-on-surface), var(--v-medium-emphasis-opacity));
  font-size: 12px;
  line-height: 1.5;
  background: rgba(var(--v-theme-on-surface), 0.04);
}
.rename-result-row > code,
.rename-result-tree {
  display: block;
  box-sizing: border-box;
  min-width: 0;
  padding: 12px 14px;
  color: rgba(var(--v-theme-on-surface), var(--v-high-emphasis-opacity));
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
    border-bottom: 1px solid rgba(var(--v-border-color), var(--v-border-opacity));
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
  background-color: rgb(var(--v-theme-surface));
  aspect-ratio: 2 / 3;
}
@media (max-width: 959px) {
  .media-poster { max-height:260px; aspect-ratio:auto; display:flex; justify-content:center; background:rgb(var(--v-theme-surface)); }
  .media-poster :deep(img) { max-height:260px; width:auto !important; max-width:100%; }
}

.poster-placeholder, .avatar-placeholder {
  width: 100%;
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  background-color: rgba(var(--v-theme-on-surface), 0.04);
}

.actor-grid-container {
  display: grid;
  grid-template-columns:repeat(auto-fit,minmax(min(100%,270px),1fr));
  gap: 16px;
}
.actor-grid-item { min-width:0; }

.actor-edit-card:hover {
  transform: translateY(-4px);
  box-shadow: var(--app-card-hover-shadow, none) !important;
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
  border-radius: var(--app-card-radius, 12px);
  flex: 0 0 72px;
}

.actor-inputs {
  display: flex;
  flex-direction: column;
  gap: 8px;
  flex-grow: 1;
  flex-basis: 0;
  min-width: 0;
}

@media (max-width:540px) {
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
  background: rgba(var(--v-theme-on-surface), 0.04);
  border: 1px dashed rgba(var(--v-border-color), var(--v-border-opacity));
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
  color: rgba(var(--v-theme-on-surface), var(--v-medium-emphasis-opacity));
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
  color: rgba(var(--v-theme-on-surface), var(--v-high-emphasis-opacity));
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
  background-color: rgb(var(--v-theme-surface));
  border: 1px solid rgba(var(--v-border-color), var(--v-border-opacity));
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
  color: rgb(var(--v-theme-on-surface));
}

.episode-season-count {
  padding: 1px 7px;
  border-radius: 999px;
  font-size: 12px;
  color: rgba(var(--v-theme-on-surface), var(--v-medium-emphasis-opacity));
  background-color: rgba(var(--v-theme-on-surface), 0.04);
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
  background-color: rgb(var(--v-theme-error));
  color: rgb(var(--v-theme-on-error));
  font-size: 10px;
  font-weight: 700;
  line-height: 14px;
  text-align: center;
}

.episode-chip {
  width: 42px;
  height: 30px;
  padding: 0;
  border: 1px solid rgba(var(--v-border-color), var(--v-border-opacity));
  border-radius: 5px;
  background-color: rgb(var(--v-theme-surface));
  color: rgb(var(--v-theme-on-surface));
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
  box-shadow: 0 0 0 2px color-mix(in srgb, rgb(var(--v-theme-primary)) 16%, transparent);
}

.episode-chip-selected .episode-chip {
  border-color: rgb(var(--v-theme-primary));
  box-shadow: 0 0 0 2px color-mix(in srgb, rgb(var(--v-theme-primary)) 12%, transparent);
}

.episode-chip-multi-version .episode-chip {
  border-color: rgb(var(--v-theme-warning));
  background-color: color-mix(in srgb, rgb(var(--v-theme-warning)) 12%, rgb(var(--v-theme-surface)));
  color: rgb(var(--v-theme-warning));
  box-shadow: 0 0 0 2px color-mix(in srgb, rgb(var(--v-theme-warning)) 14%, transparent);
}

.episode-chip-track-issue .episode-chip {
  border-color: rgb(var(--v-theme-error));
  color: rgb(var(--v-theme-error));
  box-shadow: 0 0 0 2px color-mix(in srgb, rgb(var(--v-theme-error)) 12%, transparent);
}

.episode-chip-track-unavailable .episode-chip {
  border-color: rgb(var(--v-theme-warning));
  color: rgb(var(--v-theme-warning));
  box-shadow: 0 0 0 2px color-mix(in srgb, rgb(var(--v-theme-warning)) 12%, transparent);
}

.episode-chip-track-unavailable .episode-chip-issue-dot {
  background-color: rgb(var(--v-theme-warning));
  color: rgb(var(--v-theme-on-warning));
}

.episode-chip-multi-version.episode-chip-track-issue .episode-chip {
  color: rgb(var(--v-theme-warning));
}

.episode-chip:is(:hover, .episode-chip-active) {
  border-color: rgb(var(--v-theme-primary));
  background-color: rgb(var(--v-theme-primary));
  color: rgb(var(--v-theme-on-primary));
}

.episode-chip-multi-version .episode-chip:is(:hover, .episode-chip-active) {
  border-color: rgb(var(--v-theme-warning));
  background-color: rgb(var(--v-theme-warning));
  color: rgb(var(--v-theme-on-warning));
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
  border: 1px solid rgba(var(--v-border-color), var(--v-border-opacity));
  border-radius: 6px;
  background-color: rgb(var(--v-theme-surface));
  color: rgb(var(--v-theme-on-surface));
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
  color: rgb(var(--v-theme-error));
  font-weight: 700;
  white-space: nowrap;
}

.media-info-version-filename {
  min-width: 0;
  color: rgb(var(--v-theme-on-surface));
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
  border: 1px solid rgba(var(--v-border-color), var(--v-border-opacity));
  border-radius: 6px;
  background-color: rgb(var(--v-theme-surface));
  color: rgb(var(--v-theme-on-surface));
  line-height: 1.5;
  overflow-wrap: anywhere;
}

.media-info-batch-hint {
  margin-top: 6px;
  color: rgba(var(--v-theme-on-surface), var(--v-medium-emphasis-opacity));
  font-size: 12px;
}

.stream-technical-label {
  margin-top: 4px;
  color: rgba(var(--v-theme-on-surface), var(--v-medium-emphasis-opacity));
  font-size: 11px;
  line-height: 1.25;
  white-space: nowrap;
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
  background: rgba(var(--v-border-color), var(--v-border-opacity));
  border-radius: 4px;
}
.episode-selector-board::-webkit-scrollbar-thumb:hover {
  background: rgba(var(--v-theme-on-surface), var(--v-medium-emphasis-opacity));
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
.media-image-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(min(100%, 220px), 1fr));
  gap: 20px;
  padding: 10px 0;
}

.media-image-card:hover {
  box-shadow: var(--app-card-hover-shadow, none);
  border-color: rgb(var(--v-theme-primary));
}

.image-placeholder {
  width: 100%;
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  background-color: rgba(var(--v-theme-on-surface), 0.04);
  position: absolute;
  top: 0;
  left: 0;
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

.media-tmdb-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
  gap: 24px 16px;
  padding: 10px 0;
  max-height: 65vh;
  overflow-y: auto;
  padding-right: 8px;
}

.media-tmdb-grid::-webkit-scrollbar {
  width: 6px;
}
.media-tmdb-grid::-webkit-scrollbar-track {
  background: transparent;
}
.media-tmdb-grid::-webkit-scrollbar-thumb {
  background: rgba(var(--v-border-color), var(--v-border-opacity));
  border-radius: 4px;
}
.media-tmdb-grid::-webkit-scrollbar-thumb:hover {
  background: rgba(var(--v-theme-on-surface), var(--v-medium-emphasis-opacity));
}

.media-tmdb-card {
  cursor: pointer;
  display: flex;
  flex-direction: column;
  transition: transform 0.2s;
}

.editor-actions { flex-wrap: wrap; gap: 8px; padding: 16px 24px; }
html[data-theme='glass'][data-glass-color-scheme='dark'] .media-credits-action {
  --glass-button-surface: rgba(var(--glass-material-rgb), 52%);
  --glass-button-surface-hover: rgba(var(--glass-material-rgb), 60%);
}
.actor-edit-card { height: 100%; }
.actor-edit-card :deep(.v-card-title) { padding: 12px 12px 0; font-size: 15px; }
.actor-edit-card :deep(.v-card-text) { padding: 12px; }
.media-info-feedback { max-height: 200px; overflow: auto; overflow-wrap: anywhere; }
.media-info-table-scroll :deep(.v-field) { min-width: 160px; }
.media-info-table-scroll :deep(.v-table__wrapper > table > tbody > tr > td) { padding-block: 8px; }
.batch-version-select { min-width: 240px; max-width: 100%; }
.media-poster, .actor-avatar-image, .media-image { cursor: zoom-in; }
.media-image-grid { grid-template-columns: repeat(auto-fit, minmax(min(100%, 220px), 1fr)); align-items: start; }
.media-tmdb-card { cursor: pointer; }
.episode-chip { min-width: 0; border-radius: 6px; }
.episode-chip-checkbox { inline-size: 24px; block-size: 24px; }
@media (max-width: 600px) { .editor-actions { padding: 12px 16px; } .batch-version-select { width: 100%; } }
</style>
