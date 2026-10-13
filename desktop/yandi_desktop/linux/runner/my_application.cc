#include "my_application.h"

#include <flutter_linux/flutter_linux.h>
#ifdef GDK_WINDOWING_X11
#include <gdk/gdkx.h>
#endif

#include "flutter/generated_plugin_registrant.h"

struct _MyApplication {
  GtkApplication parent_instance;
  char** dart_entrypoint_arguments;
  GtkWidget* chooser;
};

G_DEFINE_TYPE(MyApplication, my_application, GTK_TYPE_APPLICATION)

// Called when first Flutter frame received.
static void first_frame_cb(MyApplication* self, FlView* view) {
  gtk_widget_show(gtk_widget_get_toplevel(GTK_WIDGET(view)));
}

// Local file chooser is asynchronous so vault polling continues while it is open.
static void chooser_response(GtkDialog* dialog, gint response, gpointer data) {
  MyApplication* self = MY_APPLICATION(data);
  FlMethodCall* call = FL_METHOD_CALL(g_object_get_data(G_OBJECT(dialog), "call"));
  g_autofree gchar* path = response == GTK_RESPONSE_ACCEPT
      ? gtk_file_chooser_get_filename(GTK_FILE_CHOOSER(dialog)) : nullptr;
  g_autoptr(FlValue) result = path ? fl_value_new_string(path) : fl_value_new_null();
  fl_method_call_respond_success(call, result, nullptr);
  self->chooser = nullptr;
  gtk_widget_destroy(GTK_WIDGET(dialog));
}

static void local_method(FlMethodChannel*, FlMethodCall* call, gpointer data) {
  MyApplication* self = MY_APPLICATION(data);
  const gchar* method = fl_method_call_get_name(call);
  if (g_strcmp0(method, "cancelChooser") == 0) {
    if (self->chooser) gtk_dialog_response(GTK_DIALOG(self->chooser), GTK_RESPONSE_CANCEL);
    fl_method_call_respond_success(call, nullptr, nullptr);
  } else if (g_strcmp0(method, "choose") == 0) {
    if (self->chooser) {
      fl_method_call_respond_error(call, "busy", "File chooser busy", nullptr, nullptr);
      return;
    }
    FlValue* args = fl_method_call_get_args(call);
    FlValue* save_value = fl_value_lookup_string(args, "save");
    FlValue* name_value = fl_value_lookup_string(args, "name");
    gboolean save = save_value && fl_value_get_bool(save_value);
    self->chooser = gtk_file_chooser_dialog_new(save ? "Сохранить файл" : "Выбрать файл",
        GTK_WINDOW(gtk_application_get_active_window(GTK_APPLICATION(self))),
        save ? GTK_FILE_CHOOSER_ACTION_SAVE : GTK_FILE_CHOOSER_ACTION_OPEN,
        "Отмена", GTK_RESPONSE_CANCEL, save ? "Сохранить" : "Выбрать", GTK_RESPONSE_ACCEPT, nullptr);
    gtk_window_set_modal(GTK_WINDOW(self->chooser), TRUE);
    gtk_file_chooser_set_local_only(GTK_FILE_CHOOSER(self->chooser), TRUE);
    if (save && name_value) gtk_file_chooser_set_current_name(GTK_FILE_CHOOSER(self->chooser), fl_value_get_string(name_value));
    g_object_set_data_full(G_OBJECT(self->chooser), "call", g_object_ref(call), g_object_unref);
    g_signal_connect(self->chooser, "response", G_CALLBACK(chooser_response), self);
    gtk_widget_show(self->chooser);
  } else if (g_strcmp0(method, "withdrawNotification") == 0) {
    g_application_withdraw_notification(G_APPLICATION(self), "yandi-message");
    fl_method_call_respond_success(call, nullptr, nullptr);
  } else if (g_strcmp0(method, "messageAlert") == 0) {
    FlValue* args = fl_method_call_get_args(call);
    FlValue* sound = fl_value_lookup_string(args, "sound");
    FlValue* notify = fl_value_lookup_string(args, "notify");
    FlValue* english = fl_value_lookup_string(args, "english");
    if (sound && fl_value_get_bool(sound)) gdk_display_beep(gdk_display_get_default());
    if (notify && fl_value_get_bool(notify)) {
      g_autoptr(GNotification) notification = g_notification_new("YANDI");
      g_notification_set_body(notification, english && fl_value_get_bool(english)
          ? "New message" : "Новое сообщение");
      g_application_send_notification(G_APPLICATION(self), "yandi-message", notification);
    }
    fl_method_call_respond_success(call, nullptr, nullptr);
  } else if (g_strcmp0(method, "open") == 0) {
    const gchar* path = fl_value_get_string(fl_method_call_get_args(call));
    g_autofree gchar* uri = g_filename_to_uri(path, nullptr, nullptr);
    g_autoptr(GError) error = nullptr;
    if (uri && g_app_info_launch_default_for_uri(uri, nullptr, &error)) {
      fl_method_call_respond_success(call, nullptr, nullptr);
    } else {
      fl_method_call_respond_error(call, "open", "Cannot open file", nullptr, nullptr);
    }
  } else {
    fl_method_call_respond_not_implemented(call, nullptr);
  }
}

// Implements GApplication::activate.
static void my_application_activate(GApplication* application) {
  MyApplication* self = MY_APPLICATION(application);
  // Окно уже открыто (повторный запуск): не создаём второе, а поднимаем имеющееся.
  GList* windows = gtk_application_get_windows(GTK_APPLICATION(application));
  if (windows != nullptr) {
    gtk_window_present(GTK_WINDOW(windows->data));
    return;
  }
  // File selection must not write GTK recent-file history. GSettings is memory-only.
  g_object_set(gtk_settings_get_default(), "gtk-recent-files-enabled", FALSE, nullptr);
  GtkWindow* window =
      GTK_WINDOW(gtk_application_window_new(GTK_APPLICATION(application)));

  // Use a header bar when running in GNOME as this is the common style used
  // by applications and is the setup most users will be using (e.g. Ubuntu
  // desktop).
  // If running on X and not using GNOME then just use a traditional title bar
  // in case the window manager does more exotic layout, e.g. tiling.
  // If running on Wayland assume the header bar will work (may need changing
  // if future cases occur).
  gboolean use_header_bar = TRUE;
#ifdef GDK_WINDOWING_X11
  GdkScreen* screen = gtk_window_get_screen(window);
  if (GDK_IS_X11_SCREEN(screen)) {
    const gchar* wm_name = gdk_x11_screen_get_window_manager_name(screen);
    if (g_strcmp0(wm_name, "GNOME Shell") != 0) {
      use_header_bar = FALSE;
    }
  }
#endif
  if (use_header_bar) {
    GtkHeaderBar* header_bar = GTK_HEADER_BAR(gtk_header_bar_new());
    gtk_widget_show(GTK_WIDGET(header_bar));
    gtk_header_bar_set_title(header_bar, "YANDI");
    gtk_header_bar_set_show_close_button(header_bar, TRUE);
    gtk_window_set_titlebar(window, GTK_WIDGET(header_bar));
  } else {
    gtk_window_set_title(window, "YANDI");
  }

  gtk_window_set_default_size(window, 1280, 720);

  g_autoptr(FlDartProject) project = fl_dart_project_new();
  fl_dart_project_set_dart_entrypoint_arguments(
      project, self->dart_entrypoint_arguments);

  FlView* view = fl_view_new(project);
  GdkRGBA background_color;
  // Background defaults to black, override it here if necessary, e.g. #00000000
  // for transparent.
  gdk_rgba_parse(&background_color, "#000000");
  fl_view_set_background_color(view, &background_color);
  gtk_widget_show(GTK_WIDGET(view));
  gtk_container_add(GTK_CONTAINER(window), GTK_WIDGET(view));

  // Show the window when Flutter renders.
  // Requires the view to be realized so we can start rendering.
  g_signal_connect_swapped(view, "first-frame", G_CALLBACK(first_frame_cb),
                           self);
  gtk_widget_realize(GTK_WIDGET(view));

  fl_register_plugins(FL_PLUGIN_REGISTRY(view));
  g_autoptr(FlStandardMethodCodec) codec = fl_standard_method_codec_new();
  g_autoptr(FlMethodChannel) local = fl_method_channel_new(
      fl_engine_get_binary_messenger(fl_view_get_engine(view)), "yandi/local", FL_METHOD_CODEC(codec));
  fl_method_channel_set_method_call_handler(local, local_method, self, nullptr);

  gtk_widget_grab_focus(GTK_WIDGET(view));
}

// Implements GApplication::local_command_line.
static gboolean my_application_local_command_line(GApplication* application,
                                                  gchar*** arguments,
                                                  int* exit_status) {
  MyApplication* self = MY_APPLICATION(application);
  // Strip out the first argument as it is the binary name.
  self->dart_entrypoint_arguments = g_strdupv(*arguments + 1);

  g_autoptr(GError) error = nullptr;
  if (!g_application_register(application, nullptr, &error)) {
    g_warning("Failed to register: %s", error->message);
    *exit_status = 1;
    return TRUE;
  }

  g_application_activate(application);
  *exit_status = 0;

  return TRUE;
}

// Implements GApplication::startup.
static void my_application_startup(GApplication* application) {
  // MyApplication* self = MY_APPLICATION(object);

  // Perform any actions required at application startup.

  G_APPLICATION_CLASS(my_application_parent_class)->startup(application);
}

// Implements GApplication::shutdown.
static void my_application_shutdown(GApplication* application) {
  // MyApplication* self = MY_APPLICATION(object);

  // Perform any actions required at application shutdown.

  G_APPLICATION_CLASS(my_application_parent_class)->shutdown(application);
}

// Implements GObject::dispose.
static void my_application_dispose(GObject* object) {
  MyApplication* self = MY_APPLICATION(object);
  g_clear_pointer(&self->dart_entrypoint_arguments, g_strfreev);
  G_OBJECT_CLASS(my_application_parent_class)->dispose(object);
}

static void my_application_class_init(MyApplicationClass* klass) {
  G_APPLICATION_CLASS(klass)->activate = my_application_activate;
  G_APPLICATION_CLASS(klass)->local_command_line =
      my_application_local_command_line;
  G_APPLICATION_CLASS(klass)->startup = my_application_startup;
  G_APPLICATION_CLASS(klass)->shutdown = my_application_shutdown;
  G_OBJECT_CLASS(klass)->dispose = my_application_dispose;
}

static void my_application_init(MyApplication* self) {}

MyApplication* my_application_new() {
  // Set the program name to the application ID, which helps various systems
  // like GTK and desktop environments map this running application to its
  // corresponding .desktop file. This ensures better integration by allowing
  // the application to be recognized beyond its binary name.
  g_set_prgname(APPLICATION_ID);

  // The shell keeps user data in memory; disable driver shader disk caches too.
  g_setenv("MESA_SHADER_CACHE_DISABLE", "true", TRUE);
  g_setenv("__GL_SHADER_DISK_CACHE", "0", TRUE);
  g_setenv("GSETTINGS_BACKEND", "memory", TRUE);

  return MY_APPLICATION(g_object_new(my_application_get_type(),
                                     "application-id", APPLICATION_ID, "flags",
                                     G_APPLICATION_DEFAULT_FLAGS, nullptr));
}
