import { createApp } from "vue";
import App from "./App.vue";
import { initTheme } from "./theme";
import "./styles.css";

// Applied before mounting so the window opens in the saved theme rather than
// starting light and correcting itself.
initTheme();

createApp(App).mount("#app");
