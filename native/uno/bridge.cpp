// Narrow UNO type/dispatch adapter. Workbook policy and orchestration live in
// Rust.
#include <com/sun/star/beans/MethodConcept.hpp>
#include <com/sun/star/beans/PropertyConcept.hpp>
#include <com/sun/star/bridge/XUnoUrlResolver.hpp>
#include <com/sun/star/lang/XMultiComponentFactory.hpp>
#include <com/sun/star/lang/XSingleServiceFactory.hpp>
#include <com/sun/star/reflection/XIdlArray.hpp>
#include <com/sun/star/reflection/XIdlField.hpp>
#include <com/sun/star/reflection/XIdlField2.hpp>
#include <com/sun/star/reflection/XIdlMethod.hpp>
#include <com/sun/star/reflection/XIdlReflection.hpp>
#include <com/sun/star/script/XInvocation.hpp>
#include <com/sun/star/script/XTypeConverter.hpp>
#include <com/sun/star/uno/Any.hxx>
#include <cppuhelper/bootstrap.hxx>
#include <rtl/bootstrap.hxx>
#include <osl/file.hxx>
#include <iostream>
#include <map>
#include <nlohmann/json.hpp>
#include <stdexcept>
#include <string>
namespace css = com::sun::star;
using css::uno::Any;
using css::uno::Reference;
using css::uno::Sequence;
using css::uno::UNO_QUERY_THROW;
using Json = nlohmann::json;
static rtl::OUString ou(const std::string &s) {
  return rtl::OUString::fromUtf8(rtl::OString(s.data(), s.size()));
}
static std::string utf8(const rtl::OUString &s) {
  return rtl::OUStringToOString(s, RTL_TEXTENCODING_UTF8).getStr();
}
static Reference<css::uno::XComponentContext> bootstrap() {
  // Standalone adapters otherwise receive only URE types on Arch. Reflection
  // also needs the installed office API registry (Rectangle, FillDirection...).
  rtl::OUString core, office;
  if (osl::FileBase::getFileURLFromSystemPath(
          ou(OMASHEETS_LIBREOFFICE_PROGRAM "/types.rdb"), core) != osl::FileBase::E_None ||
      osl::FileBase::getFileURLFromSystemPath(
          ou(OMASHEETS_LIBREOFFICE_PROGRAM "/types/offapi.rdb"), office) != osl::FileBase::E_None)
    throw std::runtime_error("invalid installed UNO type registry path");
  rtl::Bootstrap::set(ou("UNO_TYPES"), core + ou(" ") + office);
  return cppu::defaultBootstrap_InitialComponentContext();
}
class Bridge {
  Reference<css::uno::XComponentContext>
      local = bootstrap(),
      remote;
  Reference<css::reflection::XIdlReflection> reflection;
  Reference<css::script::XTypeConverter> converter;
  std::map<unsigned, Any> objects;
  unsigned next = 1;
  Reference<css::script::XInvocation> invocation(const Any &v) {
    Sequence<Any> args(&v, 1);
    auto factory = Reference<css::lang::XSingleServiceFactory>(
        local->getServiceManager()->createInstanceWithContext(
            ou("com.sun.star.script.Invocation"), local), UNO_QUERY_THROW);
    return Reference<css::script::XInvocation>(
        factory->createInstanceWithArguments(args), UNO_QUERY_THROW);
  }
  Any object(const Json &v) {
    auto i = objects.find(v.at("$object").get<unsigned>());
    if (i == objects.end())
      throw std::runtime_error("unknown UNO object");
    return i->second;
  }
  Any decode(const Json &v, const css::uno::Type &type = css::uno::Type()) {
    Any result;
    if (v.is_object() && v.contains("$object"))
      result = object(v);
    else if (v.is_object() && v.contains("$struct")) {
      auto cls = reflection->forName(ou(v.at("$struct").get<std::string>()));
      if (!cls.is())
        throw std::runtime_error("unknown UNO struct");
      cls->createObject(result);
      for (auto i = v.at("fields").begin(); i != v.at("fields").end(); ++i) {
        auto field = Reference<css::reflection::XIdlField2>(
            cls->getField(ou(i.key())), UNO_QUERY_THROW);
        if (!field.is())
          throw std::runtime_error("unknown UNO field");
        field->set(
            result,
            decode(i.value(), css::uno::Type(field->getType()->getTypeClass(),
                                             field->getType()->getName())));
      }
    } else if (v.is_object() && v.contains("$enum")) {
      auto cls = reflection->forName(ou(v.at("$enum").get<std::string>()));
      if (!cls.is())
        throw std::runtime_error("unknown UNO enum");
      auto field = Reference<css::reflection::XIdlField2>(
          cls->getField(ou(v.at("name").get<std::string>())), UNO_QUERY_THROW);
      if (!field.is())
        throw std::runtime_error("unknown UNO enum value");
      result = field->get(Any());
    } else if (v.is_object() && v.contains("$sequence")) {
      auto cls = reflection->forName(ou(v.at("$sequence").get<std::string>()));
      if (!cls.is())
        throw std::runtime_error("unknown UNO sequence");
      cls->createObject(result);
      auto array = cls->getArray();
      const auto &items = v.at("items");
      if (items.size() > 250000)
        throw std::runtime_error("UNO sequence exceeds limit");
      array->realloc(result, items.size());
      for (unsigned i = 0; i < items.size(); ++i)
        array->set(result, i,
                   decode(items[i], css::uno::Type(
                                        cls->getComponentType()->getTypeClass(),
                                        cls->getComponentType()->getName())));
    } else if (v.is_array()) {
      if (type.getTypeClass() != css::uno::TypeClass_SEQUENCE) {
        throw std::runtime_error("array requires a UNO sequence type");
      }
      return decode(
          Json{{"$sequence", utf8(type.getTypeName())}, {"items", v}});
    } else if (v.is_boolean())
      result <<= v.get<bool>();
    else if (v.is_number())
      result <<= v.get<double>();
    else if (v.is_string())
      result <<= ou(v.get<std::string>());
    else if (!v.is_null())
      throw std::runtime_error("unsupported UNO argument");
    if (type.getTypeClass() != css::uno::TypeClass_VOID &&
        type.getTypeClass() != css::uno::TypeClass_ANY &&
        result.getValueType() != type)
      result = converter->convertTo(result, type);
    return result;
  }
  Json encode(const Any &v, unsigned depth = 0) {
    if (depth > 64)
      throw std::runtime_error("UNO result nesting exceeds limit");
    auto tc = v.getValueTypeClass();
    switch (tc) {
    case css::uno::TypeClass_VOID:
      return nullptr;
    case css::uno::TypeClass_BOOLEAN: {
      bool x = false;
      v >>= x;
      return x;
    }
    case css::uno::TypeClass_STRING: {
      rtl::OUString x;
      v >>= x;
      return utf8(x);
    }
    case css::uno::TypeClass_INTERFACE: {
      Reference<css::uno::XInterface> x;
      v >>= x;
      if (!x.is())
        return nullptr;
      if (objects.size() >= 100000)
        throw std::runtime_error("UNO object limit exceeded");
      auto id = next++;
      objects.emplace(id, v);
      return Json{{"$object", id}};
    }
    case css::uno::TypeClass_STRUCT: {
      Json fields = Json::object();
      auto cls = reflection->forName(v.getValueTypeName());
      for (auto const &field : cls->getFields())
        fields[utf8(field->getName())] = encode(
            Reference<css::reflection::XIdlField2>(field, UNO_QUERY_THROW)
                ->get(v),
            depth + 1);
      return Json{{"$struct", utf8(v.getValueTypeName())}, {"fields", fields}};
    }
    case css::uno::TypeClass_SEQUENCE: {
      auto array = reflection->forName(v.getValueTypeName())->getArray();
      auto n = array->getLen(v);
      if (n > 250000)
        throw std::runtime_error("UNO sequence exceeds limit");
      Json result = Json::array();
      for (sal_Int32 i = 0; i < n; ++i)
        result.push_back(encode(array->get(v, i), depth + 1));
      return result;
    }
    case css::uno::TypeClass_ENUM: {
      return Json{{"$enum", utf8(v.getValueTypeName())},
                  {"value", *static_cast<const sal_Int32 *>(v.getValue())}};
    }
    case css::uno::TypeClass_BYTE:
    case css::uno::TypeClass_SHORT:
    case css::uno::TypeClass_UNSIGNED_SHORT:
    case css::uno::TypeClass_LONG:
    case css::uno::TypeClass_UNSIGNED_LONG:
    case css::uno::TypeClass_HYPER: {
      auto n = converter->convertTo(v, cppu::UnoType<sal_Int64>::get());
      sal_Int64 x = 0;
      n >>= x;
      return x;
    }
    default: {
      auto n = converter->convertTo(v, cppu::UnoType<double>::get());
      double x = 0;
      n >>= x;
      return x;
    }
    }
  }

public:
  void selfTest() {
    Json rectangle{
        {"$struct", "com.sun.star.awt.Rectangle"},
        {"fields", {{"X", 12}, {"Y", 34}, {"Width", 1000}, {"Height", 2000}}}};
    if (encode(decode(rectangle))["fields"]["Width"] != 1000)
      throw std::runtime_error("UNO struct conversion failed");
    auto rect = invocation(decode(rectangle));
    if (encode(rect->getValue(ou("Width"))) != 1000)
      throw std::runtime_error("UNO invocation factory failed");
    Json sequence{{"$sequence", "[][]any"},
                  {"items", {{"Region", 20, true}, {"North", 30, false}}}};
    if (encode(decode(sequence))[0][1] != 20)
      throw std::runtime_error("UNO sequence conversion failed");
    Json value{{"$enum", "com.sun.star.sheet.FillDirection"},
               {"name", "TO_BOTTOM"}};
    if (encode(decode(value))["$enum"] != "com.sun.star.sheet.FillDirection")
      throw std::runtime_error("UNO enum conversion failed");
  }
  Bridge() {
    reflection = Reference<css::reflection::XIdlReflection>(
        local->getServiceManager()->createInstanceWithContext(
            ou("com.sun.star.reflection.CoreReflection"), local),
        UNO_QUERY_THROW);
    converter = Reference<css::script::XTypeConverter>(
        local->getServiceManager()->createInstanceWithContext(
            ou("com.sun.star.script.Converter"), local),
        UNO_QUERY_THROW);
  }
  Json run(const Json &r) {
    auto action = r.at("action").get<std::string>();
    if (action == "connect") {
      auto resolver = Reference<css::bridge::XUnoUrlResolver>(
          local->getServiceManager()->createInstanceWithContext(
              ou("com.sun.star.bridge.UnoUrlResolver"), local),
          UNO_QUERY_THROW);
      remote = Reference<css::uno::XComponentContext>(
          resolver->resolve(ou(r.at("url").get<std::string>())),
          UNO_QUERY_THROW);
      return true;
    }
    if (!remote.is())
      throw std::runtime_error("UNO is not connected");
    if (action == "service") {
      Any a;
      a <<= remote->getServiceManager()->createInstanceWithContext(
          ou(r.at("name").get<std::string>()), remote);
      return encode(a);
    }
    if (action == "release") {
      objects.erase(r.at("object").at("$object").get<unsigned>());
      return true;
    }
    auto inv = invocation(object(r.at("object")));
    auto name = ou(r.at("name").get<std::string>());
    if (action == "get")
      return encode(inv->getValue(name));
    if (action == "set") {
      auto info = inv->getIntrospection();
      auto prop = info->getProperty(name, css::beans::PropertyConcept::ALL);
      inv->setValue(name, decode(r.at("value"), prop.Type));
      return true;
    }
    if (action == "call") {
      auto method = inv->getIntrospection()->getMethod(
          name, css::beans::MethodConcept::ALL);
      auto params = method->getParameterInfos();
      auto input = r.at("arguments");
      if (input.size() != static_cast<unsigned>(params.getLength()))
        throw std::runtime_error("UNO argument count mismatch");
      Sequence<Any> args(params.getLength());
      for (sal_Int32 i = 0; i < params.getLength(); ++i)
        args[i] =
            decode(input[i], css::uno::Type(params[i].aType->getTypeClass(),
                                            params[i].aType->getName()));
      Sequence<sal_Int16> indexes;
      Sequence<Any> values;
      return encode(inv->invoke(name, args, indexes, values));
    }
    throw std::runtime_error("unsupported UNO adapter action");
  }
};
int main(int argc, char **argv) {
  if (argc == 2 && std::string(argv[1]) == "--provenance") {
    std::cout << Json{{"source_commit", OMASHEETS_SOURCE_COMMIT},
                      {"source_sha256", OMASHEETS_SOURCE_SHA256}}
              << '\n';
    return 0;
  }
  try {
    Bridge bridge;
    if (argc == 2 && std::string(argv[1]) == "--self-test") {
      bridge.selfTest();
      std::cout << "PASS: native UNO struct, sequence and enum conversion\n";
      return 0;
    }
    std::string line;
    for (;;) {
      line.clear();
      char byte;
      while (std::cin.get(byte) && byte != '\n') {
        if (line.size() == 4 * 1024 * 1024)
          return 2;
        line.push_back(byte);
      }
      if (!std::cin && line.empty())
        break;
      try {
        std::cout << Json{{"ok", true},
                          {"result", bridge.run(Json::parse(line))}}
                  << '\n';
      } catch (const css::uno::Exception &e) {
        std::cout << Json{{"ok", false},
                          {"error", utf8(e.Message).substr(0, 512)}}
                  << '\n';
      } catch (const std::exception &e) {
        std::cout << Json{{"ok", false},
                          {"error", std::string(e.what()).substr(0, 512)}}
                  << '\n';
      }
      std::cout.flush();
    }
  } catch (const css::uno::Exception &e) {
    std::cerr << utf8(e.Message).substr(0, 512) << '\n';
    return 1;
  } catch (const std::exception &e) {
    std::cerr << e.what() << '\n';
    return 1;
  }
  return 0;
}
