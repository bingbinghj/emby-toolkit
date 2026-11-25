<!-- Ace options, padding and themes follow MoviePilot-Frontend (MIT). -->
<script setup>
import { computed, useId } from 'vue';
import { useTheme } from 'vuetify';
import { VAceEditor } from 'vue3-ace-editor';
import './jinjaMode';
import 'ace-builds/src-noconflict/theme-github_dark';
import 'ace-builds/src-noconflict/theme-github_light_default';

const props = defineProps({ modelValue: String, label: String });
const emit = defineEmits(['update:modelValue']);
const labelId = useId();
const value = computed({ get: () => props.modelValue || '', set: text => emit('update:modelValue', text) });
const { global } = useTheme();
const theme = computed(() => global.current.value.dark ? 'github_dark' : 'github_light_default');
function init(editor) {
  editor.renderer.setPadding(12);
  editor.renderer.setScrollMargin(8, 8, 0, 0);
  editor.textInput.getElement().setAttribute('aria-labelledby', labelId);
}
</script>

<template>
  <div class="template-editor">
    <div :id="labelId" class="template-editor__label">{{ label }}</div>
    <VAceEditor v-model:value="value" lang="jinja2" :theme="theme" :options="{fontSize:14,tabSize:2,showLineNumbers:true,showGutter:true,useWorker:false}" :print-margin="false" :min-lines="4" :max-lines="12" wrap class="template-editor__input" @init="init" />
  </div>
</template>

<style scoped>
.template-editor{min-inline-size:0;}
.template-editor__label{font-weight:500;margin-block-end:8px;color:rgb(var(--v-theme-on-surface));}
.template-editor__input{inline-size:100%;border:1px solid var(--mp-border);border-radius:8px;line-height:1.6;}
.template-editor__input:focus-within{outline:2px solid rgb(var(--v-theme-primary));outline-offset:1px;}
.template-editor__input.ace-github-light-default :deep(.ace_keyword){color:#a31526;}
.template-editor__input.ace-github-light-default :deep(.ace_variable),.template-editor__input.ace-github-light-default :deep(.ace_constant){color:#0550ae;}
.template-editor__input.ace-github-light-default :deep(.ace_support){color:#6639ba;}
.template-editor__input.ace-github-light-default :deep(.ace_string){color:#116329;}
.template-editor__input.ace-github-light-default :deep(.ace_comment){color:#57606a;}
.template-editor__input.ace-github-dark :deep(.ace_keyword){color:#ff7b72;}
.template-editor__input.ace-github-dark :deep(.ace_variable),.template-editor__input.ace-github-dark :deep(.ace_constant){color:#79c0ff;}
.template-editor__input.ace-github-dark :deep(.ace_support){color:#d2a8ff;}
.template-editor__input.ace-github-dark :deep(.ace_string){color:#a5d6ff;}
.template-editor__input.ace-github-dark :deep(.ace_comment){color:#b1bac4;}
</style>
