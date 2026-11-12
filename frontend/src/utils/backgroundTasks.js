import axios, { AxiosError } from 'axios';
import { refreshTaskStatus } from '../composables/useTaskStatus';

// Preserve each operation's final JSON response while long work runs outside its HTTP request.
export function installBackgroundTaskInterceptor() {
  axios.interceptors.response.use(async response => {
    if (response.status !== 202 || !response.data?.task_id) return response;
    const id = response.data.task_id;
    refreshTaskStatus(true);
    let readFailures = 0;
    for (;;) {
      let task;
      try {
        ({ data: task } = await axios.get(`/api/tasks/result/${encodeURIComponent(id)}`, {
          timeout: 10000, headers: { 'Cache-Control': 'no-cache' },
        }));
        readFailures = 0;
      } catch (error) {
        const status = error.response?.status;
        const retryable = error.code !== 'ERR_CANCELED'
          && (!error.response || (status >= 500 && status < 600));
        if (!retryable) throw error;
        if (++readFailures < 3) {
          await new Promise(resolve => setTimeout(resolve, 1000));
          continue;
        }
        const message = '任务已提交，但状态尚未确认；任务可能仍在运行，请在任务中心查看结果。';
        const result = { ...response, status: status || 503, data: { task_id: id, error: message } };
        throw new AxiosError(message, 'ERR_BAD_RESPONSE', response.config, response.request, result);
      }
      if (['completed', 'completed_with_warnings', 'failed', 'cancelled'].includes(task.state)) {
        refreshTaskStatus(true);
        const completed = ['completed', 'completed_with_warnings'].includes(task.state);
        const result = { ...response, status: task.result_status || (completed ? 200 : 500), data: task.result || (completed ? { message: task.message } : { error: task.message }) };
        if (!completed) {
          result.data = { ...result.data, error: result.data.error || task.message || '任务未完成' };
          throw new AxiosError(result.data.error, 'ERR_BAD_RESPONSE', response.config, response.request, result);
        }
        return result;
      }
      await new Promise(resolve => setTimeout(resolve, 1000));
    }
  });
}
