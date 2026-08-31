#define LOK_USE_UNSTABLE_API

#ifndef OMASHEETS_SOURCE_SHA256
#define OMASHEETS_SOURCE_SHA256 "unknown"
#endif
#ifndef OMASHEETS_SOURCE_COMMIT
#define OMASHEETS_SOURCE_COMMIT "unknown"
#endif

#include <LibreOfficeKit/LibreOfficeKitGtk.h>

#include <algorithm>
#include <cctype>
#include <chrono>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <iterator>
#include <map>
#include <mutex>
#include <sstream>
#include <string>
#include <string_view>
#include <stdexcept>
#include <sys/resource.h>
#include <sys/stat.h>
#include <unistd.h>
#include <vector>

namespace fs = std::filesystem;

namespace {

struct WindowState {
    GtkWidget* window = nullptr;
    GtkWidget* view = nullptr;
    GtkWidget* scroller = nullptr;
    GtkWidget* sheets = nullptr;
    GtkWidget* address = nullptr;
    GtkWidget* formula = nullptr;
    GtkWidget* status = nullptr;
    GtkWidget* zoom = nullptr;
    GtkWidget* spinner = nullptr;
    GtkWidget* save = nullptr;
    GtkWidget* diff_button = nullptr;
    GtkWidget* diff_revealer = nullptr;
    GtkWidget* diff_summary = nullptr;
    GtkWidget* diff_list = nullptr;
    GtkWidget* diff_approve = nullptr;
    fs::path source;
    fs::path profile;
    fs::path smoke_screenshot;
    fs::path context_path;
    fs::path bridge_path;
    fs::path diff_path;
    fs::path cli_path;
    std::string session_id;
    int revision = 0;
    int sheet = 0;
    std::string selected_address;
    std::string selected_formula;
    float zoom_value = 1.0F;
    GdkRectangle visible{};
    LibreOfficeKitDocument* document = nullptr;
    GSocketService* bridge_service = nullptr;
    std::mutex bridge_mutex;
    bool loaded = false;
    bool active = false;
    bool dirty = false;
    bool smoke = false;
    bool editable = false;
    guint visible_area_source = 0;
    guint context_source = 0;
    guint diff_source = 0;
    unsigned visible_area_requests = 0;
    unsigned visible_area_updates = 0;
    unsigned scroll_events = 0;
    bool first_paint_seen = false;
    std::string diff_plan_id;
    unsigned diff_change_count = 0;
    unsigned diff_operation_count = 0;
    unsigned diff_destructive_count = 0;
    bool diff_overlay_loaded = false;
    std::chrono::steady_clock::time_point started = std::chrono::steady_clock::now();
    std::chrono::steady_clock::time_point loaded_at{};
    long load_ms = 0;
    long first_paint_ms = 0;
};

constexpr const char* kProgram = "/usr/lib/libreoffice/program";

std::string json_escape(std::string_view value)
{
    std::ostringstream escaped;
    for (const unsigned char character : value) {
        switch (character) {
        case '\\': escaped << "\\\\"; break;
        case '"': escaped << "\\\""; break;
        case '\b': escaped << "\\b"; break;
        case '\f': escaped << "\\f"; break;
        case '\n': escaped << "\\n"; break;
        case '\r': escaped << "\\r"; break;
        case '\t': escaped << "\\t"; break;
        default:
            if (character < 0x20) {
                const char* hex = "0123456789abcdef";
                escaped << "\\u00" << hex[character >> 4] << hex[character & 0x0f];
            } else {
                escaped << character;
            }
        }
    }
    return escaped.str();
}

void write_context_now(WindowState* state)
{
    if (state->context_path.empty())
        return;
    const fs::path temporary = state->context_path.string() + ".tmp-" + std::to_string(getpid());
    try {
        std::ofstream output(temporary, std::ios::trunc);
        output << "{\"active\":" << (state->active ? "true" : "false")
               << ",\"address\":\"" << json_escape(state->selected_address.substr(0, 64)) << "\""
               << ",\"dirty\":" << (state->dirty ? "true" : "false")
               << ",\"formula\":\"" << json_escape(state->selected_formula.substr(0, 8192)) << "\""
               << ",\"live_document_bridge\":" << (state->bridge_service != nullptr && state->active ? "true" : "false")
               << ",\"revision\":" << state->revision
               << ",\"session_id\":\"" << state->session_id << "\""
               << ",\"sheet\":" << state->sheet
               << ",\"updated_at_ms\":" << g_get_real_time() / 1000
               << ",\"version\":1"
               << ",\"visible\":{\"height\":" << std::max(0, state->visible.height)
               << ",\"width\":" << std::max(0, state->visible.width)
               << ",\"x\":" << std::max(0, state->visible.x)
               << ",\"y\":" << std::max(0, state->visible.y) << "}"
               << ",\"zoom\":" << state->zoom_value << "}\n";
        output.flush();
        if (!output)
            return;
        output.close();
        fs::permissions(temporary, fs::perms::owner_read | fs::perms::owner_write, fs::perm_options::replace);
        fs::rename(temporary, state->context_path);
    } catch (const fs::filesystem_error&) {
        std::error_code ignored;
        fs::remove(temporary, ignored);
    }
}

bool lowercase_hex(std::string_view value)
{
    return value.size() == 32 && std::all_of(value.begin(), value.end(), [](unsigned char character) {
        return std::isdigit(character) != 0 || (character >= 'a' && character <= 'f');
    });
}

struct DiffItem {
    std::string kind;
    std::string sheet;
    std::string range;
    std::string before;
    std::string after;
};

struct DiffOverlay {
    std::string plan_id;
    std::string status;
    unsigned operation_count = 0;
    unsigned destructive_count = 0;
    unsigned warning_count = 0;
    unsigned total_changes = 0;
    bool truncated = false;
    std::vector<DiffItem> items;
};

std::vector<std::string> split_fields(const std::string& line)
{
    std::vector<std::string> fields;
    std::size_t start = 0;
    while (true) {
        const std::size_t end = line.find('\t', start);
        fields.push_back(line.substr(start, end == std::string::npos ? end : end - start));
        if (end == std::string::npos)
            return fields;
        start = end + 1;
    }
}

std::string percent_decode(const std::string& value)
{
    gchar* decoded = g_uri_unescape_string(value.c_str(), nullptr);
    if (decoded == nullptr)
        throw std::runtime_error("invalid diff field encoding");
    std::string result(decoded);
    g_free(decoded);
    return result;
}

unsigned bounded_unsigned(const std::map<std::string, std::string>& metadata, const char* key, unsigned maximum)
{
    const auto found = metadata.find(key);
    if (found == metadata.end() || found->second.empty()
        || !std::all_of(found->second.begin(), found->second.end(), [](unsigned char character) { return std::isdigit(character) != 0; }))
        throw std::runtime_error("invalid diff metadata");
    const unsigned long parsed = std::stoul(found->second);
    if (parsed > maximum)
        throw std::runtime_error("diff metadata exceeds limit");
    return static_cast<unsigned>(parsed);
}

DiffOverlay read_diff_overlay(WindowState* state)
{
    struct stat details{};
    if (stat(state->diff_path.c_str(), &details) != 0 || !S_ISREG(details.st_mode)
        || details.st_uid != getuid() || (details.st_mode & 0077) != 0
        || details.st_size < 1 || details.st_size > 256 * 1024)
        throw std::runtime_error("unsafe diff overlay file");
    std::ifstream input(state->diff_path, std::ios::binary);
    std::string payload((std::istreambuf_iterator<char>(input)), std::istreambuf_iterator<char>());
    std::istringstream stream(payload);
    std::string line;
    if (!std::getline(stream, line) || line != "OMASHEETS_DIFF_V1")
        throw std::runtime_error("unsupported diff overlay");
    std::map<std::string, std::string> metadata;
    DiffOverlay overlay;
    while (std::getline(stream, line)) {
        if (line.empty())
            continue;
        const auto fields = split_fields(line);
        if (fields.size() == 3 && fields[0] == "meta") {
            if (!metadata.emplace(fields[1], percent_decode(fields[2])).second)
                throw std::runtime_error("duplicate diff metadata");
        } else if (fields.size() == 6 && fields[0] == "item" && overlay.items.size() < 200) {
            overlay.items.push_back({
                percent_decode(fields[1]), percent_decode(fields[2]), percent_decode(fields[3]),
                percent_decode(fields[4]), percent_decode(fields[5]),
            });
        } else {
            throw std::runtime_error("malformed diff overlay");
        }
    }
    const auto session = metadata.find("session_id");
    const auto revision = metadata.find("revision");
    const auto plan = metadata.find("plan_id");
    const auto status = metadata.find("status");
    const auto truncated = metadata.find("truncated");
    if (metadata.size() != 9 || session == metadata.end() || session->second != state->session_id
        || revision == metadata.end() || revision->second != std::to_string(state->revision)
        || plan == metadata.end() || !lowercase_hex(plan->second)
        || status == metadata.end() || status->second.size() > 32
        || truncated == metadata.end() || (truncated->second != "true" && truncated->second != "false"))
        throw std::runtime_error("diff overlay does not match this workbook session");
    overlay.plan_id = plan->second;
    overlay.status = status->second;
    overlay.operation_count = bounded_unsigned(metadata, "operation_count", 100);
    overlay.destructive_count = bounded_unsigned(metadata, "destructive_count", 100);
    overlay.warning_count = bounded_unsigned(metadata, "warning_count", 1000);
    overlay.total_changes = bounded_unsigned(metadata, "total_changes", 1'000'000);
    overlay.truncated = truncated->second == "true";
    return overlay;
}

void bridge_reply(GSocketConnection* connection, const std::string& response)
{
    GOutputStream* output = g_io_stream_get_output_stream(G_IO_STREAM(connection));
    gsize written = 0;
    GError* error = nullptr;
    g_output_stream_write_all(output, response.data(), response.size(), &written, nullptr, &error);
    g_output_stream_flush(output, nullptr, nullptr);
    g_clear_error(&error);
}

gboolean on_bridge_request(GThreadedSocketService*, GSocketConnection* connection, GObject*, gpointer data)
{
    auto* state = static_cast<WindowState*>(data);
    GSocket* socket = g_socket_connection_get_socket(connection);
    g_socket_set_timeout(socket, 5);
    GError* error = nullptr;
    GCredentials* credentials = g_socket_get_credentials(socket, &error);
    const guint64 peer = credentials != nullptr ? g_credentials_get_unix_user(credentials, &error) : G_MAXUINT64;
    if (credentials != nullptr)
        g_object_unref(credentials);
    if (error != nullptr || peer != static_cast<guint64>(getuid())) {
        g_clear_error(&error);
        bridge_reply(connection, "{\"error\":\"peer rejected\",\"ok\":false}\n");
        return TRUE;
    }

    char buffer[257]{};
    gsize used = 0;
    GInputStream* input = g_io_stream_get_input_stream(G_IO_STREAM(connection));
    while (used < sizeof(buffer) - 1) {
        const gssize count = g_input_stream_read(input, buffer + used, sizeof(buffer) - 1 - used, nullptr, &error);
        if (count <= 0)
            break;
        used += static_cast<gsize>(count);
        if (std::find(buffer, buffer + used, '\n') != buffer + used)
            break;
    }
    if (error != nullptr || used == sizeof(buffer) - 1 || used == 0 || buffer[used - 1] != '\n') {
        g_clear_error(&error);
        bridge_reply(connection, "{\"error\":\"invalid request\",\"ok\":false}\n");
        return TRUE;
    }

    std::istringstream request(std::string(buffer, used - 1));
    std::string command;
    std::string session;
    std::string nonce;
    std::string extra;
    request >> command >> session >> nonce >> extra;
    if (command != "SNAPSHOT" || session != state->session_id || !lowercase_hex(nonce) || !extra.empty()) {
        bridge_reply(connection, "{\"error\":\"invalid request\",\"ok\":false}\n");
        return TRUE;
    }

    std::string format = state->source.extension().string().substr(1);
    std::transform(format.begin(), format.end(), format.begin(), [](unsigned char character) {
        return static_cast<char>(std::tolower(character));
    });
    const fs::path destination = state->bridge_path.parent_path() /
        ("snapshot-" + session + "-" + nonce + "." + format);
    const fs::path temporary = state->bridge_path.parent_path() /
        (".snapshot-" + session + "-" + nonce + ".tmp." + format);
    bool saved = false;
    {
        std::lock_guard<std::mutex> guard(state->bridge_mutex);
        if (state->active && state->document != nullptr && !fs::exists(destination) && !fs::exists(temporary)) {
            gchar* uri = g_filename_to_uri(temporary.c_str(), nullptr, nullptr);
            saved = uri != nullptr && state->document->pClass->saveAs(
                state->document, uri, format.c_str(), nullptr) != 0;
            g_free(uri);
            if (saved) {
                try {
                    fs::permissions(temporary, fs::perms::owner_read | fs::perms::owner_write, fs::perm_options::replace);
                    fs::rename(temporary, destination);
                } catch (const fs::filesystem_error&) {
                    saved = false;
                }
            }
        }
    }
    if (!saved) {
        std::error_code ignored;
        fs::remove(temporary, ignored);
        bridge_reply(connection, "{\"error\":\"snapshot failed\",\"ok\":false}\n");
        return TRUE;
    }
    bridge_reply(connection, "{\"format\":\"" + format + "\",\"ok\":true}\n");
    return TRUE;
}

bool start_bridge(WindowState* state)
{
    if (state->bridge_path.empty())
        return true;
    try {
        if (fs::exists(state->bridge_path))
            return false;
    } catch (const fs::filesystem_error&) {
        return false;
    }
    state->bridge_service = G_SOCKET_SERVICE(g_threaded_socket_service_new(2));
    GSocketAddress* address = g_unix_socket_address_new(state->bridge_path.c_str());
    GError* error = nullptr;
    const gboolean added = g_socket_listener_add_address(
        G_SOCKET_LISTENER(state->bridge_service), address, G_SOCKET_TYPE_STREAM,
        G_SOCKET_PROTOCOL_DEFAULT, nullptr, nullptr, &error);
    g_object_unref(address);
    if (!added) {
        g_clear_error(&error);
        g_object_unref(state->bridge_service);
        state->bridge_service = nullptr;
        return false;
    }
    try {
        fs::permissions(state->bridge_path, fs::perms::owner_read | fs::perms::owner_write, fs::perm_options::replace);
    } catch (const fs::filesystem_error&) {
        g_socket_listener_close(G_SOCKET_LISTENER(state->bridge_service));
        g_object_unref(state->bridge_service);
        state->bridge_service = nullptr;
        std::error_code ignored;
        fs::remove(state->bridge_path, ignored);
        return false;
    }
    g_signal_connect(state->bridge_service, "run", G_CALLBACK(on_bridge_request), state);
    g_socket_service_start(state->bridge_service);
    return true;
}

gboolean flush_context(gpointer data)
{
    auto* state = static_cast<WindowState*>(data);
    state->context_source = 0;
    write_context_now(state);
    return G_SOURCE_REMOVE;
}

void schedule_context(WindowState* state)
{
    if (!state->context_path.empty() && state->context_source == 0)
        state->context_source = g_timeout_add(50, flush_context, state);
}

void set_status(WindowState* state, const char* message)
{
    gtk_label_set_text(GTK_LABEL(state->status), message);
}

GtkWidget* icon_button(const char* icon, const char* tooltip)
{
    GtkWidget* button = gtk_button_new_from_icon_name(icon, GTK_ICON_SIZE_BUTTON);
    gtk_widget_set_tooltip_text(button, tooltip);
    gtk_button_set_relief(GTK_BUTTON(button), GTK_RELIEF_NONE);
    return button;
}

void post_command(WindowState* state, const char* command)
{
    if (!state->loaded || !state->editable) {
        if (state->loaded)
            set_status(state, "This workbook format is read-only in v0.0.1");
        return;
    }
    lok_doc_view_post_command(LOK_DOC_VIEW(state->view), command, nullptr, FALSE);
    gtk_widget_grab_focus(state->view);
}

void on_undo(GtkButton*, gpointer data) { post_command(static_cast<WindowState*>(data), ".uno:Undo"); }
void on_redo(GtkButton*, gpointer data) { post_command(static_cast<WindowState*>(data), ".uno:Redo"); }
void on_bold(GtkButton*, gpointer data) { post_command(static_cast<WindowState*>(data), ".uno:Bold"); }
void on_italic(GtkButton*, gpointer data) { post_command(static_cast<WindowState*>(data), ".uno:Italic"); }

void update_zoom(WindowState* state, float value)
{
    lok_doc_view_set_zoom(LOK_DOC_VIEW(state->view), value);
    state->zoom_value = value;
    const int percent = static_cast<int>(value * 100.0F + 0.5F);
    const std::string label = std::to_string(percent) + "%";
    gtk_label_set_text(GTK_LABEL(state->zoom), label.c_str());
    schedule_context(state);
}

void on_zoom_in(GtkButton*, gpointer data)
{
    auto* state = static_cast<WindowState*>(data);
    update_zoom(state, std::min(5.0F, lok_doc_view_get_zoom(LOK_DOC_VIEW(state->view)) + 0.1F));
}

void on_zoom_out(GtkButton*, gpointer data)
{
    auto* state = static_cast<WindowState*>(data);
    update_zoom(state, std::max(0.25F, lok_doc_view_get_zoom(LOK_DOC_VIEW(state->view)) - 0.1F));
}

void on_zoom_reset(GtkButton*, gpointer data) { update_zoom(static_cast<WindowState*>(data), 1.0F); }

void on_copy(GtkButton*, gpointer data)
{
    auto* state = static_cast<WindowState*>(data);
    if (!state->loaded)
        return;
    gchar* used_mime = nullptr;
    gchar* text = lok_doc_view_copy_selection(
        LOK_DOC_VIEW(state->view), "text/plain;charset=utf-8", &used_mime);
    if (text != nullptr) {
        GtkClipboard* clipboard = gtk_clipboard_get(GDK_SELECTION_CLIPBOARD);
        gtk_clipboard_set_text(clipboard, text, -1);
        gtk_clipboard_store(clipboard);
        set_status(state, "Selection copied");
    }
    g_free(used_mime);
    g_free(text);
    gtk_widget_grab_focus(state->view);
}

void on_paste(GtkButton*, gpointer data)
{
    auto* state = static_cast<WindowState*>(data);
    if (!state->loaded || !state->editable)
        return;
    GtkClipboard* clipboard = gtk_clipboard_get(GDK_SELECTION_CLIPBOARD);
    gchar* text = gtk_clipboard_wait_for_text(clipboard);
    if (text != nullptr) {
        const gboolean pasted = lok_doc_view_paste(
            LOK_DOC_VIEW(state->view), "text/plain;charset=utf-8", text, std::char_traits<char>::length(text));
        set_status(state, pasted ? "Clipboard pasted into the selected cell" : "Paste was rejected by the document");
    }
    g_free(text);
    gtk_widget_grab_focus(state->view);
}

void update_visible_area_now(WindowState* state)
{
    if (!state->loaded)
        return;
    GtkAdjustment* horizontal = gtk_scrolled_window_get_hadjustment(GTK_SCROLLED_WINDOW(state->scroller));
    GtkAdjustment* vertical = gtk_scrolled_window_get_vadjustment(GTK_SCROLLED_WINDOW(state->scroller));
    GtkAllocation allocation{};
    gtk_widget_get_allocation(state->scroller, &allocation);
    GdkRectangle visible{
        static_cast<int>(lok_doc_view_pixel_to_twip(LOK_DOC_VIEW(state->view), gtk_adjustment_get_value(horizontal))),
        static_cast<int>(lok_doc_view_pixel_to_twip(LOK_DOC_VIEW(state->view), gtk_adjustment_get_value(vertical))),
        static_cast<int>(lok_doc_view_pixel_to_twip(LOK_DOC_VIEW(state->view), allocation.width)),
        static_cast<int>(lok_doc_view_pixel_to_twip(LOK_DOC_VIEW(state->view), allocation.height)),
    };
    lok_doc_view_set_visible_area(LOK_DOC_VIEW(state->view), &visible);
    state->visible = visible;
    ++state->visible_area_updates;
    schedule_context(state);
}

gboolean flush_visible_area(gpointer data)
{
    auto* state = static_cast<WindowState*>(data);
    state->visible_area_source = 0;
    update_visible_area_now(state);
    return G_SOURCE_REMOVE;
}

void schedule_visible_area(WindowState* state)
{
    if (!state->loaded)
        return;
    ++state->visible_area_requests;
    if (state->visible_area_source == 0)
        state->visible_area_source = g_timeout_add_full(G_PRIORITY_HIGH_IDLE, 16, flush_visible_area, state, nullptr);
}

void on_scroll_changed(GtkAdjustment*, gpointer data)
{
    auto* state = static_cast<WindowState*>(data);
    ++state->scroll_events;
    schedule_visible_area(state);
}

void on_scroller_size(GtkWidget*, GtkAllocation*, gpointer data)
{
    schedule_visible_area(static_cast<WindowState*>(data));
}

gboolean on_view_drawn(GtkWidget*, cairo_t*, gpointer data)
{
    auto* state = static_cast<WindowState*>(data);
    if (state->loaded && !state->first_paint_seen) {
        state->first_paint_seen = true;
        state->first_paint_ms = std::chrono::duration_cast<std::chrono::milliseconds>(
            std::chrono::steady_clock::now() - state->loaded_at).count();
    }
    return FALSE;
}

void populate_sheets(WindowState* state)
{
    gtk_combo_box_text_remove_all(GTK_COMBO_BOX_TEXT(state->sheets));
    const int parts = lok_doc_view_get_parts(LOK_DOC_VIEW(state->view));
    for (int part = 0; part < parts; ++part) {
        gchar* name = lok_doc_view_get_part_name(LOK_DOC_VIEW(state->view), part);
        gtk_combo_box_text_append_text(GTK_COMBO_BOX_TEXT(state->sheets), name != nullptr ? name : "Sheet");
        g_free(name);
    }
    if (parts > 0)
        gtk_combo_box_set_active(GTK_COMBO_BOX(state->sheets), lok_doc_view_get_part(LOK_DOC_VIEW(state->view)));
}

void on_sheet_changed(GtkComboBox* box, gpointer data)
{
    auto* state = static_cast<WindowState*>(data);
    const int part = gtk_combo_box_get_active(box);
    if (state->loaded && part >= 0 && part != lok_doc_view_get_part(LOK_DOC_VIEW(state->view))) {
        lok_doc_view_set_part(LOK_DOC_VIEW(state->view), part);
        set_status(state, "Sheet changed");
        gtk_widget_grab_focus(state->view);
    }
}

void on_part_changed(LOKDocView*, int part, gpointer data)
{
    auto* state = static_cast<WindowState*>(data);
    state->sheet = part;
    if (gtk_combo_box_get_active(GTK_COMBO_BOX(state->sheets)) != part)
        gtk_combo_box_set_active(GTK_COMBO_BOX(state->sheets), part);
    schedule_context(state);
}

void on_address_changed(LOKDocView*, const gchar* value, gpointer data)
{
    auto* state = static_cast<WindowState*>(data);
    state->selected_address = value != nullptr ? value : "";
    gtk_label_set_text(GTK_LABEL(state->address), value != nullptr && *value != '\0' ? value : "—");
    schedule_context(state);
}

void on_formula_changed(LOKDocView*, const gchar* value, gpointer data)
{
    auto* state = static_cast<WindowState*>(data);
    state->selected_formula = value != nullptr ? value : "";
    gtk_entry_set_text(GTK_ENTRY(state->formula), value != nullptr ? value : "");
    schedule_context(state);
}

void on_selection_changed(LOKDocView*, gboolean selected, gpointer data)
{
    auto* state = static_cast<WindowState*>(data);
    set_status(state, selected
        ? "Selection active"
        : (state->editable ? "Ready — click a cell and type to edit" : "Read-only workbook"));
}

void on_command_changed(LOKDocView*, const gchar* command, gpointer data)
{
    auto* state = static_cast<WindowState*>(data);
    if (command != nullptr && g_str_has_prefix(command, ".uno:ModifiedStatus=")) {
        state->dirty = g_str_has_suffix(command, "true");
        std::string title = state->source.filename().string();
        if (state->dirty)
            title += " •";
        gtk_header_bar_set_title(GTK_HEADER_BAR(gtk_window_get_titlebar(GTK_WINDOW(state->window))), title.c_str());
        schedule_context(state);
    }
}

void on_command_result(LOKDocView*, const gchar* result, gpointer data)
{
    auto* state = static_cast<WindowState*>(data);
    if (result != nullptr && std::string_view(result).find("save") != std::string_view::npos)
        set_status(state, "LibreOfficeKit completed the save command");
}

void on_destroy(GtkWidget*, gpointer data)
{
    auto* state = static_cast<WindowState*>(data);
    {
        std::lock_guard<std::mutex> guard(state->bridge_mutex);
        state->active = false;
    }
    if (state->bridge_service != nullptr) {
        g_socket_service_stop(state->bridge_service);
        g_socket_listener_close(G_SOCKET_LISTENER(state->bridge_service));
    }
    if (state->context_source != 0) {
        g_source_remove(state->context_source);
        state->context_source = 0;
    }
    if (state->diff_source != 0) {
        g_source_remove(state->diff_source);
        state->diff_source = 0;
    }
    write_context_now(state);
    std::error_code ignored;
    if (!state->bridge_path.empty())
        fs::remove(state->bridge_path, ignored);
    gtk_main_quit();
}

void save_copy(WindowState* state, const fs::path& destination)
{
    if (fs::exists(destination)) {
        set_status(state, "Save cancelled: destination already exists");
        return;
    }
    LibreOfficeKitDocument* document = lok_doc_view_get_document(LOK_DOC_VIEW(state->view));
    gchar* uri = g_filename_to_uri(destination.c_str(), nullptr, nullptr);
    const bool saved = document != nullptr && uri != nullptr &&
        document->pClass->saveAs(document, uri, nullptr, nullptr) != 0;
    g_free(uri);
    if (saved) {
        state->dirty = false;
        gtk_header_bar_set_title(
            GTK_HEADER_BAR(gtk_window_get_titlebar(GTK_WINDOW(state->window))),
            state->source.filename().c_str());
        schedule_context(state);
    }
    set_status(state, saved ? "Saved a new workbook copy" : "LibreOfficeKit could not save the copy");
}

void on_save_copy(GtkButton*, gpointer data)
{
    auto* state = static_cast<WindowState*>(data);
    if (!state->loaded)
        return;
    GtkWidget* chooser = gtk_file_chooser_dialog_new(
        "Save an OmaSheets copy", GTK_WINDOW(state->window), GTK_FILE_CHOOSER_ACTION_SAVE,
        "Cancel", GTK_RESPONSE_CANCEL, "Save Copy", GTK_RESPONSE_ACCEPT, nullptr);
    gtk_file_chooser_set_do_overwrite_confirmation(GTK_FILE_CHOOSER(chooser), TRUE);
    const std::string suggested = state->source.stem().string() + "-omasheets" + state->source.extension().string();
    gtk_file_chooser_set_current_name(GTK_FILE_CHOOSER(chooser), suggested.c_str());
    if (gtk_dialog_run(GTK_DIALOG(chooser)) == GTK_RESPONSE_ACCEPT) {
        gchar* selected = gtk_file_chooser_get_filename(GTK_FILE_CHOOSER(chooser));
        if (selected != nullptr)
            save_copy(state, fs::path(selected));
        g_free(selected);
    }
    gtk_widget_destroy(chooser);
}

gboolean on_delete(GtkWidget*, GdkEvent*, gpointer data)
{
    auto* state = static_cast<WindowState*>(data);
    if (!state->dirty)
        return FALSE;
    GtkWidget* dialog = gtk_message_dialog_new(
        GTK_WINDOW(state->window), GTK_DIALOG_MODAL, GTK_MESSAGE_WARNING, GTK_BUTTONS_NONE,
        "Discard unsaved changes?");
    gtk_message_dialog_format_secondary_text(
        GTK_MESSAGE_DIALOG(dialog), "OmaSheets keeps the original workbook unchanged until you save a copy.");
    gtk_dialog_add_buttons(GTK_DIALOG(dialog), "Keep Editing", GTK_RESPONSE_CANCEL, "Discard", GTK_RESPONSE_ACCEPT, nullptr);
    const bool keep_open = gtk_dialog_run(GTK_DIALOG(dialog)) != GTK_RESPONSE_ACCEPT;
    gtk_widget_destroy(dialog);
    return keep_open ? TRUE : FALSE;
}

void on_password(LOKDocView* view, const gchar* url, gboolean modify, gpointer data)
{
    auto* state = static_cast<WindowState*>(data);
    GtkWidget* dialog = gtk_dialog_new_with_buttons(
        modify ? "Password to edit" : "Password required", GTK_WINDOW(state->window), GTK_DIALOG_MODAL,
        "Cancel", GTK_RESPONSE_CANCEL, "Unlock", GTK_RESPONSE_ACCEPT, nullptr);
    GtkWidget* entry = gtk_entry_new();
    gtk_entry_set_visibility(GTK_ENTRY(entry), FALSE);
    gtk_entry_set_placeholder_text(GTK_ENTRY(entry), "Workbook password");
    gtk_container_add(GTK_CONTAINER(gtk_dialog_get_content_area(GTK_DIALOG(dialog))), entry);
    gtk_widget_show(entry);
    const gchar* password = nullptr;
    if (gtk_dialog_run(GTK_DIALOG(dialog)) == GTK_RESPONSE_ACCEPT)
        password = gtk_entry_get_text(GTK_ENTRY(entry));
    lok_doc_view_set_document_password(view, url, password);
    gtk_widget_destroy(dialog);
}

gboolean capture_smoke(gpointer data)
{
    auto* state = static_cast<WindowState*>(data);
    GdkWindow* window = gtk_widget_get_window(state->window);
    const int width = gtk_widget_get_allocated_width(state->window);
    const int height = gtk_widget_get_allocated_height(state->window);
    GdkPixbuf* pixels = gdk_pixbuf_get_from_window(window, 0, 0, width, height);
    GError* error = nullptr;
    const gboolean saved = pixels != nullptr && gdk_pixbuf_save(pixels, state->smoke_screenshot.c_str(), "png", &error, nullptr);
    if (pixels != nullptr)
        g_object_unref(pixels);
    if (!saved) {
        std::cerr << "omasheets-window: screenshot failed: " << (error != nullptr ? error->message : "unknown error") << '\n';
        g_clear_error(&error);
    } else {
        rusage usage{};
        getrusage(RUSAGE_SELF, &usage);
        const double rss_mib = static_cast<double>(usage.ru_maxrss) / 1024.0;
        std::cout << "{\"editable\":" << (state->editable ? "true" : "false")
                  << ",\"engine\":\"libreofficekitgtk\",\"loaded\":true,\"parts\":"
                  << lok_doc_view_get_parts(LOK_DOC_VIEW(state->view))
                  << ",\"scrolling\":true,\"selection\":true,\"window_owned_by\":\"omasheets\""
                  << ",\"diff_overlay\":" << (state->diff_overlay_loaded ? "true" : "false")
                  << ",\"diff_change_count\":" << state->diff_change_count
                  << ",\"performance\":{\"load_ms\":" << state->load_ms
                  << ",\"first_paint_ms\":" << state->first_paint_ms
                  << ",\"first_paint_observed\":" << (state->first_paint_seen ? "true" : "false")
                  << ",\"rss_mib\":" << rss_mib
                  << ",\"scroll_events\":" << state->scroll_events
                  << ",\"visible_area_requests\":" << state->visible_area_requests
                  << ",\"visible_area_updates\":" << state->visible_area_updates << "}}\n";
    }
    gtk_widget_destroy(state->window);
    return G_SOURCE_REMOVE;
}

gboolean exercise_large_sheet_scroll(gpointer data)
{
    auto* state = static_cast<WindowState*>(data);
    GtkAdjustment* horizontal = gtk_scrolled_window_get_hadjustment(GTK_SCROLLED_WINDOW(state->scroller));
    GtkAdjustment* vertical = gtk_scrolled_window_get_vadjustment(GTK_SCROLLED_WINDOW(state->scroller));
    gtk_adjustment_set_value(horizontal, std::max(0.0, gtk_adjustment_get_upper(horizontal) * 0.75 - gtk_adjustment_get_page_size(horizontal)));
    gtk_adjustment_set_value(vertical, std::max(0.0, gtk_adjustment_get_upper(vertical) * 0.75 - gtk_adjustment_get_page_size(vertical)));
    return G_SOURCE_REMOVE;
}

void on_opened(GObject* object, GAsyncResult* result, gpointer data)
{
    auto* state = static_cast<WindowState*>(data);
    GError* error = nullptr;
    if (!lok_doc_view_open_document_finish(LOK_DOC_VIEW(object), result, &error)) {
        const std::string message = "Could not open workbook: " + std::string(error != nullptr ? error->message : "unknown error");
        set_status(state, message.c_str());
        g_clear_error(&error);
        if (state->smoke)
            gtk_widget_destroy(state->window);
        return;
    }
    state->loaded = true;
    state->document = lok_doc_view_get_document(LOK_DOC_VIEW(state->view));
    {
        std::lock_guard<std::mutex> guard(state->bridge_mutex);
        state->active = true;
    }
    state->loaded_at = std::chrono::steady_clock::now();
    state->load_ms = std::chrono::duration_cast<std::chrono::milliseconds>(state->loaded_at - state->started).count();
    lok_doc_view_set_edit(LOK_DOC_VIEW(state->view), state->editable ? TRUE : FALSE);
    populate_sheets(state);
    update_zoom(state, 1.0F);
    update_visible_area_now(state);
    gtk_spinner_stop(GTK_SPINNER(state->spinner));
    gtk_widget_hide(state->spinner);
    gtk_widget_set_sensitive(state->save, state->editable ? TRUE : FALSE);
    set_status(state, state->editable
        ? "Ready — click a cell and type to edit"
        : "Read-only format — use the verified conversion flow to create an editable .xlsx");
    gtk_widget_grab_focus(state->view);
    if (state->smoke && state->editable) {
        constexpr std::string_view marker = "OmaSheets unsaved live bridge";
        if (lok_doc_view_paste(
                LOK_DOC_VIEW(state->view), "text/plain;charset=utf-8",
                marker.data(), marker.size())) {
            state->dirty = true;
        }
    }
    if (!start_bridge(state)) {
        set_status(state, "Workbook opened, but the private agent bridge could not start");
        if (state->smoke) {
            gtk_widget_destroy(state->window);
            return;
        }
    }
    schedule_context(state);
    if (state->smoke) {
        g_timeout_add(100, exercise_large_sheet_scroll, state);
        g_timeout_add(1500, capture_smoke, state);
    }
}

void destroy_child(GtkWidget* widget, gpointer)
{
    gtk_widget_destroy(widget);
}

GtkWidget* diff_text(const std::string& text, const char* style)
{
    GtkWidget* label = gtk_label_new(text.c_str());
    gtk_label_set_xalign(GTK_LABEL(label), 0.0F);
    gtk_label_set_line_wrap(GTK_LABEL(label), TRUE);
    gtk_label_set_line_wrap_mode(GTK_LABEL(label), PANGO_WRAP_WORD_CHAR);
    gtk_label_set_max_width_chars(GTK_LABEL(label), 46);
    gtk_label_set_selectable(GTK_LABEL(label), TRUE);
    gtk_style_context_add_class(gtk_widget_get_style_context(label), style);
    return label;
}

void show_diff_overlay(WindowState* state, const DiffOverlay& overlay)
{
    gtk_container_foreach(GTK_CONTAINER(state->diff_list), destroy_child, nullptr);
    for (const DiffItem& item : overlay.items) {
        GtkWidget* card = gtk_box_new(GTK_ORIENTATION_VERTICAL, 4);
        gtk_style_context_add_class(gtk_widget_get_style_context(card), "omasheets-diff-card");
        const std::string location = item.sheet + " · " + item.range;
        GtkWidget* title = diff_text(location, "omasheets-diff-location");
        GtkWidget* before = diff_text("− " + item.before, "omasheets-diff-before");
        GtkWidget* after = diff_text("+ " + item.after, "omasheets-diff-after");
        gtk_widget_set_tooltip_text(title, item.kind.c_str());
        gtk_box_pack_start(GTK_BOX(card), title, FALSE, FALSE, 0);
        gtk_box_pack_start(GTK_BOX(card), before, FALSE, FALSE, 0);
        gtk_box_pack_start(GTK_BOX(card), after, FALSE, FALSE, 0);
        gtk_box_pack_start(GTK_BOX(state->diff_list), card, FALSE, FALSE, 0);
    }
    std::ostringstream summary;
    summary << overlay.total_changes << (overlay.total_changes == 1 ? " verified change" : " verified changes")
            << " from " << overlay.operation_count << (overlay.operation_count == 1 ? " operation" : " operations");
    if (overlay.destructive_count != 0)
        summary << " · " << overlay.destructive_count << " destructive";
    if (overlay.warning_count != 0)
        summary << " · " << overlay.warning_count << " warnings";
    if (overlay.truncated)
        summary << " · showing first 200";
    gtk_label_set_text(GTK_LABEL(state->diff_summary), summary.str().c_str());
    const std::string button = "Review " + std::to_string(overlay.total_changes) + " agent changes";
    gtk_button_set_label(GTK_BUTTON(state->diff_button), button.c_str());
    gtk_widget_show(state->diff_button);
    gtk_widget_show_all(state->diff_list);
    gtk_revealer_set_reveal_child(GTK_REVEALER(state->diff_revealer), TRUE);
    state->diff_plan_id = overlay.plan_id;
    state->diff_change_count = overlay.total_changes;
    state->diff_operation_count = overlay.operation_count;
    state->diff_destructive_count = overlay.destructive_count;
    state->diff_overlay_loaded = true;
    set_status(state, "Agent proposal ready — review overlay is read-only");
}

gboolean poll_diff_overlay(gpointer data)
{
    auto* state = static_cast<WindowState*>(data);
    if (state->diff_path.empty())
        return G_SOURCE_CONTINUE;
    if (!fs::exists(state->diff_path)) {
        if (state->diff_overlay_loaded) {
            gtk_revealer_set_reveal_child(GTK_REVEALER(state->diff_revealer), FALSE);
            gtk_widget_hide(state->diff_button);
            state->diff_plan_id.clear();
            state->diff_change_count = 0;
            state->diff_operation_count = 0;
            state->diff_destructive_count = 0;
            state->diff_overlay_loaded = false;
        }
        return G_SOURCE_CONTINUE;
    }
    try {
        const DiffOverlay overlay = read_diff_overlay(state);
        if (!state->diff_overlay_loaded || overlay.plan_id != state->diff_plan_id)
            show_diff_overlay(state, overlay);
    } catch (const std::exception&) {
        // An atomic replacement can be observed between stat and open. Invalid
        // or foreign payloads remain invisible and are retried on the next tick.
    }
    return G_SOURCE_CONTINUE;
}

void on_diff_toggle(GtkButton*, gpointer data)
{
    auto* state = static_cast<WindowState*>(data);
    const gboolean shown = gtk_revealer_get_reveal_child(GTK_REVEALER(state->diff_revealer));
    gtk_revealer_set_reveal_child(GTK_REVEALER(state->diff_revealer), !shown);
}

void on_diff_dismiss(GtkButton*, gpointer data)
{
    auto* state = static_cast<WindowState*>(data);
    gtk_revealer_set_reveal_child(GTK_REVEALER(state->diff_revealer), FALSE);
    set_status(state, "Agent proposal hidden — workbook remains unchanged");
}

void on_diff_approve(GtkButton*, gpointer data)
{
    auto* state = static_cast<WindowState*>(data);
    if (!state->diff_overlay_loaded || state->cli_path.empty())
        return;
    GtkWidget* chooser = gtk_file_chooser_dialog_new(
        "Save the approved agent copy", GTK_WINDOW(state->window), GTK_FILE_CHOOSER_ACTION_SAVE,
        "Cancel", GTK_RESPONSE_CANCEL, "Choose Copy", GTK_RESPONSE_ACCEPT, nullptr);
    gtk_file_chooser_set_do_overwrite_confirmation(GTK_FILE_CHOOSER(chooser), FALSE);
    const std::string suggested = state->source.stem().string() + "-agent-reviewed" + state->source.extension().string();
    gtk_file_chooser_set_current_name(GTK_FILE_CHOOSER(chooser), suggested.c_str());
    if (gtk_dialog_run(GTK_DIALOG(chooser)) != GTK_RESPONSE_ACCEPT) {
        gtk_widget_destroy(chooser);
        return;
    }
    gchar* selected = gtk_file_chooser_get_filename(GTK_FILE_CHOOSER(chooser));
    const fs::path destination = selected != nullptr ? fs::path(selected) : fs::path();
    g_free(selected);
    gtk_widget_destroy(chooser);
    if (destination.empty() || destination == state->source
        || destination.extension() != state->source.extension() || fs::exists(destination)) {
        GtkWidget* error = gtk_message_dialog_new(
            GTK_WINDOW(state->window), GTK_DIALOG_MODAL, GTK_MESSAGE_WARNING, GTK_BUTTONS_CLOSE,
            "Choose a new, unused %s file. The open workbook cannot be replaced.",
            state->source.extension().string().c_str());
        gtk_dialog_run(GTK_DIALOG(error));
        gtk_widget_destroy(error);
        return;
    }
    std::ostringstream detail;
    detail << state->diff_change_count << " verified changes from " << state->diff_operation_count
           << " operations will be written to a new workbook copy.";
    if (state->diff_destructive_count != 0)
        detail << " The proposal contains " << state->diff_destructive_count << " destructive operations.";
    GtkWidget* confirmation = gtk_message_dialog_new(
        GTK_WINDOW(state->window), GTK_DIALOG_MODAL, GTK_MESSAGE_QUESTION, GTK_BUTTONS_NONE,
        "Approve this agent proposal?");
    gtk_message_dialog_format_secondary_text(
        GTK_MESSAGE_DIALOG(confirmation), "%s\n\nThe open workbook remains unchanged.", detail.str().c_str());
    gtk_dialog_add_button(GTK_DIALOG(confirmation), "Cancel", GTK_RESPONSE_CANCEL);
    gtk_dialog_add_button(GTK_DIALOG(confirmation), "Approve & Save Copy", GTK_RESPONSE_ACCEPT);
    const bool approved = gtk_dialog_run(GTK_DIALOG(confirmation)) == GTK_RESPONSE_ACCEPT;
    gtk_widget_destroy(confirmation);
    if (!approved)
        return;
    set_status(state, "Publishing verified agent copy…");
    const std::string revision = std::to_string(state->revision);
    const std::string target = destination.string();
    gchar* arguments[] = {
        const_cast<gchar*>(state->cli_path.c_str()), const_cast<gchar*>("plan"),
        const_cast<gchar*>("publish-copy-native"), const_cast<gchar*>(state->diff_plan_id.c_str()),
        const_cast<gchar*>("--revision"), const_cast<gchar*>(revision.c_str()),
        const_cast<gchar*>("--destination"), const_cast<gchar*>(target.c_str()), nullptr,
    };
    GError* error = nullptr;
    gint wait_status = 0;
    const gboolean spawned = g_spawn_sync(
        nullptr, arguments, nullptr,
        static_cast<GSpawnFlags>(G_SPAWN_STDOUT_TO_DEV_NULL | G_SPAWN_STDERR_TO_DEV_NULL),
        nullptr, nullptr, nullptr, nullptr, &wait_status, &error);
    const gboolean succeeded = spawned && g_spawn_check_wait_status(wait_status, &error);
    if (succeeded) {
        gtk_revealer_set_reveal_child(GTK_REVEALER(state->diff_revealer), FALSE);
        set_status(state, "Approved agent proposal saved as a new copy");
    } else {
        GtkWidget* failure = gtk_message_dialog_new(
            GTK_WINDOW(state->window), GTK_DIALOG_MODAL, GTK_MESSAGE_ERROR, GTK_BUTTONS_CLOSE,
            "The verified copy was not published");
        gtk_message_dialog_format_secondary_text(
            GTK_MESSAGE_DIALOG(failure), "%s", error != nullptr ? error->message : "OmaSheets rejected the review");
        gtk_dialog_run(GTK_DIALOG(failure));
        gtk_widget_destroy(failure);
    }
    g_clear_error(&error);
}

void apply_style()
{
    const char* css =
        "window { background: #101512; color: #e7eee9; }"
        "headerbar { background: #162019; color: #f4f8f5; border-bottom: 1px solid #2d3b31; }"
        ".omasheets-toolbar { background: #131a16; border-bottom: 1px solid #263129; padding: 6px; }"
        ".omasheets-formula { background: #0f1511; border-bottom: 1px solid #263129; padding: 6px; }"
        ".omasheets-address { color: #7ee2a8; font-weight: bold; min-width: 72px; }"
        ".omasheets-status { color: #9fb1a5; padding: 5px 10px; }"
        ".omasheets-diff-panel { background: #111814; border-left: 1px solid #34453a; padding: 14px; }"
        ".omasheets-diff-title { font-size: 18px; font-weight: bold; color: #f4f8f5; }"
        ".omasheets-diff-summary { color: #aebdb3; padding-bottom: 8px; }"
        ".omasheets-diff-card { background: #18221c; border: 1px solid #2b3a30; border-radius: 8px; padding: 10px; margin: 0 0 8px 0; }"
        ".omasheets-diff-location { color: #dce7df; font-weight: bold; }"
        ".omasheets-diff-before { color: #ef9a9a; background: #291718; padding: 4px 6px; }"
        ".omasheets-diff-after { color: #8fe4ae; background: #102619; padding: 4px 6px; }"
        ".omasheets-review-button { background: #275f3d; color: #f7fff9; font-weight: bold; }"
        "button { border-radius: 7px; }"
        "combobox button { background: #1a251e; color: #e7eee9; }";
    GtkCssProvider* provider = gtk_css_provider_new();
    gtk_css_provider_load_from_data(provider, css, -1, nullptr);
    gtk_style_context_add_provider_for_screen(
        gdk_screen_get_default(), GTK_STYLE_PROVIDER(provider), GTK_STYLE_PROVIDER_PRIORITY_APPLICATION);
    g_object_unref(provider);
}

GtkWidget* build_window(WindowState* state)
{
    GtkWidget* window = gtk_window_new(GTK_WINDOW_TOPLEVEL);
    state->window = window;
    gtk_window_set_default_size(GTK_WINDOW(window), 1280, 800);
    gtk_window_set_position(GTK_WINDOW(window), GTK_WIN_POS_CENTER);
    gtk_window_set_role(GTK_WINDOW(window), "omasheets-workbook");

    GtkWidget* header = gtk_header_bar_new();
    gtk_header_bar_set_show_close_button(GTK_HEADER_BAR(header), TRUE);
    gtk_header_bar_set_title(GTK_HEADER_BAR(header), state->source.filename().c_str());
    gtk_header_bar_set_subtitle(GTK_HEADER_BAR(header), "OmaSheets · LibreOfficeKit engine");
    gtk_window_set_titlebar(GTK_WINDOW(window), header);

    state->save = gtk_button_new_with_label("Save a Copy");
    gtk_widget_set_tooltip_text(state->save, "Save edited workbook bytes to a new file");
    gtk_widget_set_sensitive(state->save, FALSE);
    gtk_header_bar_pack_end(GTK_HEADER_BAR(header), state->save);
    g_signal_connect(state->save, "clicked", G_CALLBACK(on_save_copy), state);
    state->diff_button = gtk_button_new_with_label("Review agent changes");
    gtk_style_context_add_class(gtk_widget_get_style_context(state->diff_button), "omasheets-review-button");
    gtk_widget_set_tooltip_text(state->diff_button, "Show the verified agent proposal without changing the workbook");
    gtk_widget_set_no_show_all(state->diff_button, TRUE);
    gtk_widget_hide(state->diff_button);
    gtk_header_bar_pack_end(GTK_HEADER_BAR(header), state->diff_button);
    g_signal_connect(state->diff_button, "clicked", G_CALLBACK(on_diff_toggle), state);
    state->spinner = gtk_spinner_new();
    gtk_spinner_start(GTK_SPINNER(state->spinner));
    gtk_header_bar_pack_end(GTK_HEADER_BAR(header), state->spinner);

    GtkWidget* root = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
    gtk_container_add(GTK_CONTAINER(window), root);

    GtkWidget* toolbar = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
    gtk_style_context_add_class(gtk_widget_get_style_context(toolbar), "omasheets-toolbar");
    gtk_box_pack_start(GTK_BOX(root), toolbar, FALSE, FALSE, 0);
    struct Action { const char* icon; const char* tooltip; GCallback callback; };
    const Action actions[] = {
        {"edit-undo-symbolic", "Undo", G_CALLBACK(on_undo)},
        {"edit-redo-symbolic", "Redo", G_CALLBACK(on_redo)},
        {"edit-copy-symbolic", "Copy selection", G_CALLBACK(on_copy)},
        {"edit-paste-symbolic", "Paste into selection", G_CALLBACK(on_paste)},
        {"format-text-bold-symbolic", "Bold", G_CALLBACK(on_bold)},
        {"format-text-italic-symbolic", "Italic", G_CALLBACK(on_italic)},
    };
    for (const Action& action : actions) {
        GtkWidget* button = icon_button(action.icon, action.tooltip);
        gtk_box_pack_start(GTK_BOX(toolbar), button, FALSE, FALSE, 0);
        g_signal_connect(button, "clicked", action.callback, state);
    }
    GtkWidget* spacer = gtk_separator_new(GTK_ORIENTATION_VERTICAL);
    gtk_box_pack_start(GTK_BOX(toolbar), spacer, FALSE, FALSE, 4);
    GtkWidget* zoom_out = icon_button("zoom-out-symbolic", "Zoom out");
    state->zoom = gtk_label_new("100%");
    GtkWidget* zoom_reset = gtk_button_new();
    gtk_container_add(GTK_CONTAINER(zoom_reset), state->zoom);
    gtk_button_set_relief(GTK_BUTTON(zoom_reset), GTK_RELIEF_NONE);
    GtkWidget* zoom_in = icon_button("zoom-in-symbolic", "Zoom in");
    gtk_box_pack_start(GTK_BOX(toolbar), zoom_out, FALSE, FALSE, 0);
    gtk_box_pack_start(GTK_BOX(toolbar), zoom_reset, FALSE, FALSE, 0);
    gtk_box_pack_start(GTK_BOX(toolbar), zoom_in, FALSE, FALSE, 0);
    g_signal_connect(zoom_out, "clicked", G_CALLBACK(on_zoom_out), state);
    g_signal_connect(zoom_reset, "clicked", G_CALLBACK(on_zoom_reset), state);
    g_signal_connect(zoom_in, "clicked", G_CALLBACK(on_zoom_in), state);

    GtkWidget* formula_bar = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8);
    gtk_style_context_add_class(gtk_widget_get_style_context(formula_bar), "omasheets-formula");
    state->address = gtk_label_new("—");
    gtk_style_context_add_class(gtk_widget_get_style_context(state->address), "omasheets-address");
    state->formula = gtk_entry_new();
    gtk_editable_set_editable(GTK_EDITABLE(state->formula), FALSE);
    gtk_entry_set_placeholder_text(GTK_ENTRY(state->formula), "Selected cell value or formula");
    gtk_box_pack_start(GTK_BOX(formula_bar), state->address, FALSE, FALSE, 0);
    gtk_box_pack_start(GTK_BOX(formula_bar), state->formula, TRUE, TRUE, 0);
    gtk_box_pack_start(GTK_BOX(root), formula_bar, FALSE, FALSE, 0);

    state->scroller = gtk_scrolled_window_new(nullptr, nullptr);
    gtk_scrolled_window_set_policy(GTK_SCROLLED_WINDOW(state->scroller), GTK_POLICY_AUTOMATIC, GTK_POLICY_AUTOMATIC);
    gtk_widget_set_hexpand(state->scroller, TRUE);
    gtk_widget_set_vexpand(state->scroller, TRUE);
    gtk_container_add(GTK_CONTAINER(state->scroller), state->view);
    GtkWidget* canvas = gtk_overlay_new();
    gtk_container_add(GTK_CONTAINER(canvas), state->scroller);
    state->diff_revealer = gtk_revealer_new();
    gtk_revealer_set_transition_type(GTK_REVEALER(state->diff_revealer), GTK_REVEALER_TRANSITION_TYPE_SLIDE_LEFT);
    gtk_revealer_set_transition_duration(GTK_REVEALER(state->diff_revealer), 180);
    gtk_widget_set_halign(state->diff_revealer, GTK_ALIGN_END);
    gtk_widget_set_valign(state->diff_revealer, GTK_ALIGN_FILL);
    GtkWidget* diff_panel = gtk_box_new(GTK_ORIENTATION_VERTICAL, 8);
    gtk_widget_set_size_request(diff_panel, 420, -1);
    gtk_style_context_add_class(gtk_widget_get_style_context(diff_panel), "omasheets-diff-panel");
    GtkWidget* diff_header = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8);
    GtkWidget* diff_title = gtk_label_new("Agent proposal");
    gtk_label_set_xalign(GTK_LABEL(diff_title), 0.0F);
    gtk_style_context_add_class(gtk_widget_get_style_context(diff_title), "omasheets-diff-title");
    GtkWidget* diff_close = icon_button("window-close-symbolic", "Hide proposal");
    gtk_box_pack_start(GTK_BOX(diff_header), diff_title, TRUE, TRUE, 0);
    gtk_box_pack_end(GTK_BOX(diff_header), diff_close, FALSE, FALSE, 0);
    state->diff_summary = gtk_label_new("No proposal");
    gtk_label_set_xalign(GTK_LABEL(state->diff_summary), 0.0F);
    gtk_label_set_line_wrap(GTK_LABEL(state->diff_summary), TRUE);
    gtk_style_context_add_class(gtk_widget_get_style_context(state->diff_summary), "omasheets-diff-summary");
    GtkWidget* diff_scroll = gtk_scrolled_window_new(nullptr, nullptr);
    gtk_scrolled_window_set_policy(GTK_SCROLLED_WINDOW(diff_scroll), GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
    state->diff_list = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
    gtk_container_add(GTK_CONTAINER(diff_scroll), state->diff_list);
    gtk_box_pack_start(GTK_BOX(diff_panel), diff_header, FALSE, FALSE, 0);
    gtk_box_pack_start(GTK_BOX(diff_panel), state->diff_summary, FALSE, FALSE, 0);
    gtk_box_pack_start(GTK_BOX(diff_panel), diff_scroll, TRUE, TRUE, 0);
    GtkWidget* boundary = gtk_label_new("Review only · the open workbook is unchanged");
    gtk_label_set_xalign(GTK_LABEL(boundary), 0.0F);
    gtk_style_context_add_class(gtk_widget_get_style_context(boundary), "omasheets-diff-summary");
    state->diff_approve = gtk_button_new_with_label("Approve & Save a Copy…");
    gtk_style_context_add_class(gtk_widget_get_style_context(state->diff_approve), "omasheets-review-button");
    gtk_widget_set_tooltip_text(state->diff_approve, "Confirm and publish the verified proposal to a new workbook file");
    gtk_box_pack_end(GTK_BOX(diff_panel), state->diff_approve, FALSE, FALSE, 0);
    gtk_box_pack_end(GTK_BOX(diff_panel), boundary, FALSE, FALSE, 0);
    gtk_container_add(GTK_CONTAINER(state->diff_revealer), diff_panel);
    gtk_overlay_add_overlay(GTK_OVERLAY(canvas), state->diff_revealer);
    gtk_box_pack_start(GTK_BOX(root), canvas, TRUE, TRUE, 0);
    g_signal_connect(diff_close, "clicked", G_CALLBACK(on_diff_dismiss), state);
    g_signal_connect(state->diff_approve, "clicked", G_CALLBACK(on_diff_approve), state);

    GtkWidget* footer = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8);
    state->sheets = gtk_combo_box_text_new();
    gtk_widget_set_tooltip_text(state->sheets, "Active sheet");
    state->status = gtk_label_new("Loading workbook…");
    gtk_widget_set_halign(state->status, GTK_ALIGN_START);
    gtk_style_context_add_class(gtk_widget_get_style_context(state->status), "omasheets-status");
    gtk_box_pack_start(GTK_BOX(footer), state->sheets, FALSE, FALSE, 6);
    gtk_box_pack_start(GTK_BOX(footer), state->status, TRUE, TRUE, 0);
    gtk_box_pack_end(GTK_BOX(footer), gtk_label_new("Experimental native window"), FALSE, FALSE, 10);
    gtk_box_pack_start(GTK_BOX(root), footer, FALSE, FALSE, 0);

    g_signal_connect(window, "destroy", G_CALLBACK(on_destroy), state);
    g_signal_connect(window, "delete-event", G_CALLBACK(on_delete), state);
    g_signal_connect(state->sheets, "changed", G_CALLBACK(on_sheet_changed), state);
    g_signal_connect(state->view, "part-changed", G_CALLBACK(on_part_changed), state);
    g_signal_connect(state->view, "address-changed", G_CALLBACK(on_address_changed), state);
    g_signal_connect(state->view, "formula-changed", G_CALLBACK(on_formula_changed), state);
    g_signal_connect(state->view, "text-selection", G_CALLBACK(on_selection_changed), state);
    g_signal_connect(state->view, "command-changed", G_CALLBACK(on_command_changed), state);
    g_signal_connect(state->view, "command-result", G_CALLBACK(on_command_result), state);
    g_signal_connect(state->view, "password-required", G_CALLBACK(on_password), state);
    g_signal_connect_after(state->view, "draw", G_CALLBACK(on_view_drawn), state);
    g_signal_connect(gtk_scrolled_window_get_hadjustment(GTK_SCROLLED_WINDOW(state->scroller)), "value-changed", G_CALLBACK(on_scroll_changed), state);
    g_signal_connect(gtk_scrolled_window_get_vadjustment(GTK_SCROLLED_WINDOW(state->scroller)), "value-changed", G_CALLBACK(on_scroll_changed), state);
    g_signal_connect(state->scroller, "size-allocate", G_CALLBACK(on_scroller_size), state);
    return window;
}

bool supported(const fs::path& path)
{
    std::string extension = path.extension().string();
    std::transform(extension.begin(), extension.end(), extension.begin(), [](unsigned char c) { return static_cast<char>(std::tolower(c)); });
    return extension == ".xls" || extension == ".xlsx" || extension == ".xlsm" || extension == ".ods";
}

}  // namespace

int main(int argc, char** argv)
{
    if (argc == 2 && std::string_view(argv[1]) == "--provenance") {
        std::cout << "{\"component\":\"omasheets-window\",\"source_commit\":\""
                  << OMASHEETS_SOURCE_COMMIT << "\",\"source_sha256\":\""
                  << OMASHEETS_SOURCE_SHA256 << "\"}\n";
        return 0;
    }
    gtk_init(&argc, &argv);
    WindowState state;
    std::string stage = "arguments";
    int source_index = -1;
    for (int index = 1; index < argc; ++index) {
        const std::string_view argument(argv[index]);
        if (argument == "--smoke-test" && index + 1 < argc) {
            state.smoke = true;
            state.smoke_screenshot = fs::absolute(argv[++index]);
        } else if (argument == "--context" && index + 1 < argc) {
            state.context_path = fs::absolute(argv[++index]);
        } else if (argument == "--bridge" && index + 1 < argc) {
            state.bridge_path = fs::absolute(argv[++index]);
        } else if (argument == "--diff" && index + 1 < argc) {
            state.diff_path = fs::absolute(argv[++index]);
        } else if (argument == "--cli" && index + 1 < argc) {
            state.cli_path = fs::absolute(argv[++index]);
        } else if (argument == "--session" && index + 1 < argc) {
            state.session_id = argv[++index];
        } else if (argument == "--revision" && index + 1 < argc) {
            state.revision = std::atoi(argv[++index]);
        } else if (!argument.empty() && argument.front() != '-' && source_index < 0) {
            source_index = index;
        } else {
            source_index = -1;
            break;
        }
    }
    const bool any_context = !state.context_path.empty() || !state.bridge_path.empty() || !state.diff_path.empty() || !state.cli_path.empty() || !state.session_id.empty() || state.revision != 0;
    if (source_index < 0 || (any_context && (state.context_path.empty() || state.bridge_path.empty() || state.diff_path.empty() || state.cli_path.empty() || !lowercase_hex(state.session_id) || state.revision < 1))) {
        std::cerr << "usage: omasheets-window [--smoke-test SCREENSHOT.png] [--context FILE --bridge SOCKET --diff OVERLAY --cli EXECUTABLE --session ID --revision N] WORKBOOK\n";
        return 2;
    }
    try {
        stage = "workbook validation";
        state.source = fs::canonical(argv[source_index]);
        if (!fs::is_regular_file(state.source) || !supported(state.source))
            throw std::runtime_error("input must be a regular .xls, .xlsx, .xlsm, or .ods workbook");
        if (any_context && (!fs::is_regular_file(state.cli_path) || access(state.cli_path.c_str(), X_OK) != 0))
            throw std::runtime_error("OmaSheets CLI executable was not found");
        std::string extension = state.source.extension().string();
        std::transform(extension.begin(), extension.end(), extension.begin(), [](unsigned char c) { return static_cast<char>(std::tolower(c)); });
        state.editable = extension == ".xlsx" || extension == ".ods";
        const char* configured = std::getenv("OMASHEETS_LOK_PROGRAM");
        const fs::path program = configured != nullptr ? fs::path(configured) : fs::path(kProgram);
        if (!fs::is_directory(program))
            throw std::runtime_error("LibreOffice program directory was not found");
        stage = "profile creation";
        GError* error = nullptr;
        gchar* profile = g_dir_make_tmp("omasheets-window-XXXXXX", &error);
        if (profile == nullptr)
            throw std::runtime_error(error != nullptr ? error->message : "cannot create isolated profile");
        state.profile = profile;
        g_free(profile);
        stage = "profile URI";
        gchar* profile_uri = g_filename_to_uri(state.profile.c_str(), nullptr, &error);
        if (profile_uri == nullptr)
            throw std::runtime_error(error != nullptr ? error->message : "cannot create profile URI");
        stage = "LibreOfficeKitGTK initialization";
        state.view = lok_doc_view_new_from_user_profile(program.c_str(), profile_uri, nullptr, &error);
        g_free(profile_uri);
        if (state.view == nullptr)
            throw std::runtime_error(error != nullptr ? error->message : "LibreOfficeKitGTK initialization failed");
        stage = "OmaSheets styling";
        apply_style();
        stage = "window construction";
        GtkWidget* window = build_window(&state);
        gtk_widget_show_all(window);
        gtk_revealer_set_reveal_child(GTK_REVEALER(state.diff_revealer), FALSE);
        if (!state.diff_path.empty()) {
            poll_diff_overlay(&state);
            state.diff_source = g_timeout_add(200, poll_diff_overlay, &state);
        }
        stage = "workbook URI";
        gchar* source_uri = g_filename_to_uri(state.source.c_str(), nullptr, &error);
        if (source_uri == nullptr)
            throw std::runtime_error(error != nullptr ? error->message : "cannot create workbook URI");
        stage = "asynchronous workbook open";
        lok_doc_view_open_document(
            // LibreOfficeKitGTK's post-load path always parses this argument as
            // JSON (including when no rendering options are needed).
            LOK_DOC_VIEW(state.view), source_uri, "{}", nullptr, on_opened, &state);
        stage = "interactive event loop";
        gtk_main();
        std::error_code ignored;
        fs::remove_all(state.profile, ignored);
        return state.smoke && !state.loaded ? 1 : 0;
    } catch (const std::exception& exception) {
        std::cerr << "omasheets-window (" << stage << "): " << exception.what() << '\n';
        std::error_code ignored;
        if (!state.profile.empty())
            fs::remove_all(state.profile, ignored);
        return 1;
    }
}
