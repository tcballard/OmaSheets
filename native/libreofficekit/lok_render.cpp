#define LOK_USE_UNSTABLE_API

#ifndef OMASHEETS_SOURCE_SHA256
#define OMASHEETS_SOURCE_SHA256 "unknown"
#endif
#ifndef OMASHEETS_SOURCE_COMMIT
#define OMASHEETS_SOURCE_COMMIT "unknown"
#endif

#include <LibreOfficeKit/LibreOfficeKit.hxx>

#include <algorithm>
#include <array>
#include <cerrno>
#include <cstdint>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <iomanip>
#include <iostream>
#include <memory>
#include <sstream>
#include <stdexcept>
#include <string>
#include <string_view>
#include <system_error>
#include <vector>

namespace fs = std::filesystem;

namespace {

constexpr int kDefaultWidth = 1024;
constexpr int kDefaultHeight = 640;
constexpr int kMaxDimension = 4096;
constexpr long kDefaultTileWidthTwips = 14400;
constexpr long kDefaultTileHeightTwips = 9000;

struct TemporaryProfile {
    fs::path path;

    TemporaryProfile()
    {
        std::array<char, 64> pattern{};
        const std::string base = (fs::temp_directory_path() / "omasheets-lok-XXXXXX").string();
        if (base.size() + 1 > pattern.size())
            throw std::runtime_error("temporary directory path is too long");
        std::copy(base.begin(), base.end(), pattern.begin());
        char* created = ::mkdtemp(pattern.data());
        if (created == nullptr)
            throw std::runtime_error("cannot create isolated LibreOffice profile");
        path = created;
    }

    ~TemporaryProfile()
    {
        std::error_code ignored;
        fs::remove_all(path, ignored);
    }

    TemporaryProfile(const TemporaryProfile&) = delete;
    TemporaryProfile& operator=(const TemporaryProfile&) = delete;
};

std::string json_escape(std::string_view value)
{
    std::ostringstream escaped;
    for (const unsigned char character : value) {
        switch (character) {
            case '\\': escaped << "\\\\"; break;
            case '"': escaped << "\\\""; break;
            case '\n': escaped << "\\n"; break;
            case '\r': escaped << "\\r"; break;
            case '\t': escaped << "\\t"; break;
            default:
                if (character < 0x20)
                    escaped << "\\u" << std::hex << std::setw(4) << std::setfill('0')
                            << static_cast<int>(character) << std::dec;
                else
                    escaped << character;
        }
    }
    return escaped.str();
}

bool uri_unreserved(unsigned char character)
{
    return (character >= 'a' && character <= 'z') ||
           (character >= 'A' && character <= 'Z') ||
           (character >= '0' && character <= '9') ||
           character == '-' || character == '.' || character == '_' || character == '~' || character == '/';
}

std::string file_uri(const fs::path& path)
{
    const std::string absolute = fs::absolute(path).lexically_normal().string();
    std::ostringstream uri;
    uri << "file://";
    for (const unsigned char character : absolute) {
        if (uri_unreserved(character))
            uri << character;
        else
            uri << '%' << std::uppercase << std::hex << std::setw(2) << std::setfill('0')
                << static_cast<int>(character) << std::nouppercase << std::dec;
    }
    return uri.str();
}

int positive_dimension(const char* raw, std::string_view name)
{
    std::size_t consumed = 0;
    const int value = std::stoi(raw, &consumed);
    if (consumed != std::string_view(raw).size() || value < 1 || value > kMaxDimension)
        throw std::runtime_error(std::string(name) + " must be between 1 and 4096");
    return value;
}

void write_ppm(
    const fs::path& destination,
    const std::vector<unsigned char>& pixels,
    int width,
    int height,
    LibreOfficeKitTileMode mode)
{
    fs::path temporary = destination;
    temporary += ".omasheets-tmp";
    if (fs::exists(temporary))
        throw std::runtime_error("temporary output already exists: " + temporary.string());

    std::ofstream output(temporary, std::ios::binary | std::ios::trunc);
    if (!output)
        throw std::runtime_error("cannot create output: " + temporary.string());
    output << "P6\n" << width << ' ' << height << "\n255\n";
    for (std::size_t offset = 0; offset < pixels.size(); offset += 4) {
        if (mode == LOK_TILEMODE_BGRA) {
            output.put(static_cast<char>(pixels[offset + 2]));
            output.put(static_cast<char>(pixels[offset + 1]));
            output.put(static_cast<char>(pixels[offset]));
        } else {
            output.put(static_cast<char>(pixels[offset]));
            output.put(static_cast<char>(pixels[offset + 1]));
            output.put(static_cast<char>(pixels[offset + 2]));
        }
    }
    output.close();
    if (!output) {
        std::error_code ignored;
        fs::remove(temporary, ignored);
        throw std::runtime_error("failed while writing output: " + temporary.string());
    }
    fs::rename(temporary, destination);
}

std::string office_error(lok::Office& office)
{
    char* raw = office.getError();
    if (raw == nullptr)
        return "unknown LibreOfficeKit error";
    const std::string message(raw);
    office.freeError(raw);
    return message;
}

}  // namespace

int main(int argc, char** argv)
{
    try {
        if (argc == 2 && std::string_view(argv[1]) == "--provenance") {
            std::cout << "{\"component\":\"omasheets-lok-render\",\"source_commit\":\""
                      << OMASHEETS_SOURCE_COMMIT << "\",\"source_sha256\":\""
                      << OMASHEETS_SOURCE_SHA256 << "\"}\n";
            return 0;
        }
        if (argc < 3 || argc > 5) {
            std::cerr << "usage: omasheets-lok-render INPUT.{xls,xlsx,xlsm,ods} OUTPUT.ppm [WIDTH HEIGHT]\n";
            return 2;
        }

        const fs::path source = fs::canonical(argv[1]);
        const fs::path destination = fs::absolute(argv[2]).lexically_normal();
        const int width = argc >= 4 ? positive_dimension(argv[3], "width") : kDefaultWidth;
        const int height = argc >= 5 ? positive_dimension(argv[4], "height") : kDefaultHeight;
        if (!fs::is_regular_file(source))
            throw std::runtime_error("input is not a regular file");
        if (source == destination)
            throw std::runtime_error("input and output paths must differ");
        if (destination.extension() != ".ppm")
            throw std::runtime_error("spike output must use the .ppm extension");
        if (fs::exists(destination))
            throw std::runtime_error("output already exists");
        if (!destination.parent_path().empty() && !fs::is_directory(destination.parent_path()))
            throw std::runtime_error("output directory does not exist");

        const char* configured_program = std::getenv("OMASHEETS_LOK_PROGRAM");
        const fs::path program = configured_program != nullptr
            ? fs::path(configured_program)
            : fs::path("/usr/lib/libreoffice/program");
        if (!fs::is_directory(program))
            throw std::runtime_error("LibreOffice program directory not found: " + program.string());

        TemporaryProfile profile;
        const std::string profile_uri = file_uri(profile.path);
        std::unique_ptr<lok::Office> office(lok::lok_cpp_init(program.c_str(), profile_uri.c_str()));
        if (!office)
            throw std::runtime_error("LibreOfficeKit initialization failed");

        const std::string source_uri = file_uri(source);
        std::unique_ptr<lok::Document> document(office->documentLoad(source_uri.c_str(), "Language=en-US"));
        if (!document)
            throw std::runtime_error("document load failed: " + office_error(*office));
        if (document->getDocumentType() != LOK_DOCTYPE_SPREADSHEET)
            throw std::runtime_error("input is not a spreadsheet document");

        document->initializeForRendering();
        long document_width = 0;
        long document_height = 0;
        document->getDocumentSize(&document_width, &document_height);
        if (document_width <= 0 || document_height <= 0)
            throw std::runtime_error("LibreOfficeKit reported an empty document canvas");

        const long tile_width = std::min(document_width, kDefaultTileWidthTwips);
        const long tile_height = std::min(document_height, kDefaultTileHeightTwips);
        std::vector<unsigned char> pixels(static_cast<std::size_t>(width) * height * 4);
        document->paintTile(pixels.data(), width, height, 0, 0, tile_width, tile_height);
        const auto tile_mode = static_cast<LibreOfficeKitTileMode>(document->getTileMode());
        if (tile_mode != LOK_TILEMODE_BGRA && tile_mode != LOK_TILEMODE_RGBA)
            throw std::runtime_error("LibreOfficeKit returned an unsupported tile pixel format");
        write_ppm(destination, pixels, width, height, tile_mode);

        std::cout << "{\"document_height_twips\":" << document_height
                  << ",\"document_width_twips\":" << document_width
                  << ",\"engine\":\"libreofficekit\""
                  << ",\"height\":" << height
                  << ",\"output\":\"" << json_escape(destination.string()) << "\""
                  << ",\"parts\":" << document->getParts()
                  << ",\"source\":\"" << json_escape(source.string()) << "\""
                  << ",\"tile_mode\":\"" << (tile_mode == LOK_TILEMODE_BGRA ? "BGRA" : "RGBA") << "\""
                  << ",\"width\":" << width << "}\n";
        return 0;
    } catch (const std::exception& error) {
        std::cerr << "omasheets-lok-render: " << error.what() << '\n';
        return 1;
    }
}
