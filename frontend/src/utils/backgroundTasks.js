import axios, { AxiosError } from 'axios';
import { refreshTaskStatus } from '../composables/useTaskStatus';

// Preserve each operation's final JSON response while long work runs outside its HTTP request.
export function installBackgroundTaskInterceptor() {
  axios.interceptors.response.use(async response => {
    if (response.status !== 202 || !response.data?.task_id) return response;
    const id = response.data.task_id;
    refreshTaskStatus();
    for (;;) {
      const { data: task } = await axios.get(`/api/tasks/result/${encodeURIComponent(id)}`);
      if (['completed', 'completed_with_warnings', 'failed', 'cancelled'].includes(task.state)) {
        refreshTaskStatus();
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
